#!/usr/bin/env python3
"""Run matched end-to-end image-generation benchmarks for Rust and Python."""

from __future__ import annotations

import argparse
import gc
import hashlib
import json
import math
import os
import platform
import random
import statistics
import subprocess
import sys
import time
import tomllib
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

# Native thread limits must be set before NumPy loads its backends.
for thread_variable in (
    "OMP_NUM_THREADS",
    "OPENBLAS_NUM_THREADS",
    "MKL_NUM_THREADS",
    "VECLIB_MAXIMUM_THREADS",
):
    os.environ[thread_variable] = "1"

import numpy as np  # noqa: E402
import PIL  # noqa: E402
from PIL import features  # noqa: E402
import wordcloud as python_wordcloud  # noqa: E402
from wordcloud import WordCloud  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]
FONT_PATH = ROOT / "assets" / "OpenSans-Regular.ttf"
SEED = 42
PREFIXES = (
    "adaptive",
    "atomic",
    "binary",
    "cloud",
    "compute",
    "concurrent",
    "data",
    "dynamic",
    "efficient",
    "fast",
    "graphic",
    "indexed",
    "layout",
    "memory",
    "parallel",
    "pixel",
)
SUFFIXES = (
    "engine",
    "buffer",
    "system",
    "render",
    "vector",
    "matrix",
    "service",
    "runtime",
    "module",
    "stream",
    "kernel",
    "thread",
    "cache",
    "image",
    "signal",
    "model",
)


@dataclass(frozen=True)
class Case:
    name: str
    width: int
    height: int
    candidates: int
    min_font_size: int
    max_font_size: int
    relative_scaling: float
    masked: bool = False


CASES = (
    Case("fixed_sparse_400x200", 400, 200, 20, 12, 12, 0.0),
    Case("fixed_standard_800x400", 800, 400, 100, 10, 10, 0.0),
    Case("fixed_dense_1200x600", 1200, 600, 200, 10, 10, 0.0),
    Case("fixed_mask_800x600", 800, 600, 100, 10, 10, 0.0, True),
    Case("weighted_800x400", 800, 400, 100, 4, 128, 0.5),
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--warmup", type=positive_int, default=3)
    parser.add_argument("--samples", type=positive_int, default=15)
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "benchmarks" / "results" / "latest.json",
        help="raw JSON result path",
    )
    return parser.parse_args()


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return parsed


def frequencies(count: int) -> list[tuple[str, int]]:
    if count > len(PREFIXES) * len(SUFFIXES):
        raise ValueError("the deterministic workload supports at most 256 words")
    return [
        (
            PREFIXES[index % len(PREFIXES)] + SUFFIXES[index // len(PREFIXES)],
            1_000_000 // (index + 1),
        )
        for index in range(count)
    ]


def workload_checksum(items: Iterable[tuple[str, int]]) -> str:
    value = 0xCBF29CE484222325
    for word, frequency in items:
        for byte in f"{word}={frequency}\n".encode("ascii"):
            value ^= byte
            value = (value * 0x00000100000001B3) & 0xFFFF_FFFF_FFFF_FFFF
    return f"{value:016x}"


def solid_black(*_args: Any, **_kwargs: Any) -> tuple[int, int, int, int]:
    return (0, 0, 0, 255)


def make_mask(case: Case) -> np.ndarray[Any, np.dtype[np.uint8]] | None:
    if not case.masked:
        return None
    y, x = np.ogrid[: case.height, : case.width]
    dx = x.astype(np.int64) * 2 + 1 - case.width
    dy = y.astype(np.int64) * 2 + 1 - case.height
    inside_ellipse = (
        dx * dx * case.height * case.height + dy * dy * case.width * case.width
        <= case.width * case.width * case.height * case.height
    )
    top_notch = (np.abs(dx) < case.width // 6) & (dy < -case.height // 3)
    return np.where(inside_ellipse & ~top_notch, 0, 255).astype(np.uint8)


def mask_checksum(mask: np.ndarray[Any, np.dtype[np.uint8]] | None) -> str:
    if mask is None:
        return "none"
    value = 0xCBF29CE484222325
    for byte in mask.tobytes(order="C"):
        value ^= byte
        value = (value * 0x00000100000001B3) & 0xFFFF_FFFF_FFFF_FFFF
    return f"{value:016x}"


def build_python_cloud(
    case: Case, mask: np.ndarray[Any, np.dtype[np.uint8]] | None
) -> WordCloud:
    return WordCloud(
        font_path=str(FONT_PATH),
        width=case.width,
        height=case.height,
        scale=1,
        max_words=case.candidates,
        min_font_size=case.min_font_size,
        max_font_size=case.max_font_size,
        font_step=2,
        margin=2,
        prefer_horizontal=1.0,
        relative_scaling=case.relative_scaling,
        random_state=SEED,
        background_color=(255, 255, 255, 255),
        mode="RGBA",
        color_func=solid_black,
        repeat=False,
        contour_width=0,
        mask=mask,
    )


def measure_python(case: Case, warmup: int, samples: int) -> dict[str, Any]:
    items = frequencies(case.candidates)
    frequency_map = dict(items)
    mask = make_mask(case)
    cloud = build_python_cloud(case, mask)
    initial_random_state = random.Random(SEED).getstate()

    def generate_once() -> tuple[int, int, tuple[int, int], str, Any]:
        cloud.random_state.setstate(initial_random_state)
        started = time.perf_counter_ns()
        image = cloud.generate_from_frequencies(frequency_map).to_image()
        elapsed_ns = time.perf_counter_ns() - started
        placed = len(cloud.layout_)
        dimensions = image.size
        mode = image.mode
        return elapsed_ns, placed, dimensions, mode, image

    reference_render = None
    for _ in range(warmup):
        _, placed, dimensions, mode, image = generate_once()
        validate_python_render(case, placed, dimensions, mode)
        if reference_render is None:
            reference_render = render_metrics(image, cloud.layout_)
        del image
        cloud.layout_ = []

    times_ns: list[int] = []
    placed_words: list[int] = []
    gc.collect()
    gc_was_enabled = gc.isenabled()
    gc.disable()
    try:
        for _ in range(samples):
            elapsed_ns, placed, dimensions, mode, image = generate_once()
            validate_python_render(case, placed, dimensions, mode)
            times_ns.append(elapsed_ns)
            placed_words.append(placed)
            del image
            cloud.layout_ = []
    finally:
        if gc_was_enabled:
            gc.enable()

    return {
        "workload_checksum": workload_checksum(items),
        "mask_checksum": mask_checksum(mask),
        "render_metrics": reference_render,
        "times_ns": times_ns,
        "placed_words": placed_words,
        "summary": summarize(times_ns, placed_words),
    }


def render_metrics(image: Any, layout: list[Any]) -> dict[str, Any]:
    font_sizes = [entry[1] for entry in layout]
    pixels = np.asarray(image)
    ink_pixels = int(np.count_nonzero(np.any(pixels != 255, axis=2)))
    return {
        "placed_words": len(layout),
        "min_font_size": min(font_sizes),
        "max_font_size": max(font_sizes),
        "font_size_sum": sum(font_sizes),
        "ink_pixels": ink_pixels,
    }


def validate_python_render(
    case: Case, placed: int, dimensions: tuple[int, int], mode: str
) -> None:
    if dimensions != (case.width, case.height):
        raise RuntimeError(
            f"{case.name} produced {dimensions}, expected {(case.width, case.height)}"
        )
    if mode != "RGBA":
        raise RuntimeError(f"{case.name} produced image mode {mode}, expected RGBA")
    if not 0 < placed <= case.candidates:
        raise RuntimeError(f"{case.name} placed an invalid number of words: {placed}")


def run_rust(warmup: int, samples: int) -> dict[str, dict[str, Any]]:
    command = [
        "cargo",
        "bench",
        "--quiet",
        "--bench",
        "comparison",
        "--",
        "--warmup",
        str(warmup),
        "--samples",
        str(samples),
    ]
    print("$ " + " ".join(command), flush=True)
    completed = subprocess.run(
        command,
        cwd=ROOT,
        env=single_threaded_environment(),
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        sys.stderr.write(completed.stdout)
        sys.stderr.write(completed.stderr)
        raise RuntimeError(f"Rust benchmark exited with {completed.returncode}")
    if completed.stderr.strip():
        sys.stderr.write(completed.stderr)

    results: dict[str, dict[str, Any]] = {}
    for line in completed.stdout.splitlines():
        parts = line.split("\t")
        if parts[0] == "RUST_CASE" and len(parts) == 11:
            name = parts[1]
            results[name] = {
                "width": int(parts[2]),
                "height": int(parts[3]),
                "candidates": int(parts[4]),
                "min_font_size": float(parts[5]),
                "max_font_size": float(parts[6]),
                "relative_scaling": float(parts[7]),
                "masked": bool(int(parts[8])),
                "workload_checksum": parts[9],
                "mask_checksum": parts[10],
                "times_ns": [],
                "placed_words": [],
            }
        elif parts[0] == "RUST_RENDER" and len(parts) == 7:
            name = parts[1]
            if name not in results:
                raise RuntimeError(f"render metadata appeared before case: {name}")
            results[name]["render_metrics"] = {
                "placed_words": int(parts[2]),
                "min_font_size": float(parts[3]),
                "max_font_size": float(parts[4]),
                "font_size_sum": float(parts[5]),
                "ink_pixels": int(parts[6]),
            }
        elif parts[0] == "RUST_SAMPLE" and len(parts) == 4:
            name = parts[1]
            if name not in results:
                raise RuntimeError(f"sample appeared before case metadata: {name}")
            results[name]["times_ns"].append(int(parts[2]))
            results[name]["placed_words"].append(int(parts[3]))

    for case in CASES:
        result = results.get(case.name)
        if result is None:
            raise RuntimeError(f"Rust benchmark did not report {case.name}")
        if len(result["times_ns"]) != samples:
            raise RuntimeError(
                f"Rust benchmark reported {len(result['times_ns'])} samples "
                f"for {case.name}, expected {samples}"
            )
        expected = asdict(case)
        for field in ("width", "height", "candidates", "masked"):
            if result[field] != expected[field]:
                raise RuntimeError(f"Rust metadata mismatch for {case.name}.{field}")
        for field in ("min_font_size", "max_font_size", "relative_scaling"):
            if result[field] != expected[field]:
                raise RuntimeError(f"Rust metadata mismatch for {case.name}.{field}")
        if "render_metrics" not in result:
            raise RuntimeError(
                f"Rust benchmark did not report render metrics for {case.name}"
            )
        result["summary"] = summarize(result["times_ns"], result["placed_words"])
    return results


def merge_rust_results(
    before: dict[str, dict[str, Any]], after: dict[str, dict[str, Any]]
) -> dict[str, dict[str, Any]]:
    merged: dict[str, dict[str, Any]] = {}
    for case in CASES:
        first = before[case.name]
        second = after[case.name]
        metadata_fields = (
            "width",
            "height",
            "candidates",
            "min_font_size",
            "max_font_size",
            "relative_scaling",
            "masked",
            "workload_checksum",
            "mask_checksum",
            "render_metrics",
        )
        for field in metadata_fields:
            if first[field] != second[field]:
                raise RuntimeError(
                    f"Rust before/after metadata mismatch for {case.name}.{field}"
                )
        combined = {field: first[field] for field in metadata_fields}
        combined["times_ns"] = first["times_ns"] + second["times_ns"]
        combined["placed_words"] = first["placed_words"] + second["placed_words"]
        combined["summary"] = summarize(combined["times_ns"], combined["placed_words"])
        merged[case.name] = combined
    return merged


def single_threaded_environment() -> dict[str, str]:
    environment = os.environ.copy()
    environment.update(
        {
            "OMP_NUM_THREADS": "1",
            "OPENBLAS_NUM_THREADS": "1",
            "MKL_NUM_THREADS": "1",
            "VECLIB_MAXIMUM_THREADS": "1",
        }
    )
    return environment


def summarize(times_ns: list[int], placed_words: list[int]) -> dict[str, Any]:
    ordered = sorted(times_ns)
    median_ns = statistics.median(ordered)
    deviations = [abs(value - median_ns) for value in ordered]
    p95_ns = ordered[max(0, math.ceil(len(ordered) * 0.95) - 1)]
    median_placed = statistics.median(placed_words)
    return {
        "median_ms": median_ns / 1_000_000,
        "mean_ms": statistics.mean(ordered) / 1_000_000,
        "min_ms": ordered[0] / 1_000_000,
        "max_ms": ordered[-1] / 1_000_000,
        "p95_ms": p95_ns / 1_000_000,
        "mad_ms": statistics.median(deviations) / 1_000_000,
        "stdev_ms": statistics.pstdev(ordered) / 1_000_000,
        "median_placed_words": median_placed,
        "min_placed_words": min(placed_words),
        "max_placed_words": max(placed_words),
        "median_ms_per_placed_word": (median_ns / 1_000_000) / median_placed,
    }


def command_output(command: list[str]) -> str | None:
    try:
        completed = subprocess.run(
            command,
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError:
        return None
    if completed.returncode != 0:
        return None
    return completed.stdout.strip() or None


def machine_metadata() -> dict[str, Any]:
    cpu = command_output(["sysctl", "-n", "machdep.cpu.brand_string"])
    memory = command_output(["sysctl", "-n", "hw.memsize"])
    return {
        "os": platform.platform(),
        "architecture": platform.machine(),
        "cpu": cpu or platform.processor() or "unknown",
        "logical_cpus": os.cpu_count(),
        "memory_bytes": int(memory) if memory and memory.isdigit() else None,
    }


def version_metadata() -> dict[str, Any]:
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    return {
        "wordcloud_rust": manifest["package"]["version"],
        "rustc": command_output(["rustc", "-Vv"]),
        "cargo": command_output(["cargo", "-V"]),
        "python": sys.version,
        "python_wordcloud": python_wordcloud.__version__,
        "numpy": np.__version__,
        "pillow": PIL.__version__,
        "freetype": features.version("freetype2"),
    }


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_tree(paths: Iterable[Path]) -> str:
    digest = hashlib.sha256()
    for path in sorted(paths):
        relative_path = path.relative_to(ROOT).as_posix().encode("utf-8")
        content = path.read_bytes()
        digest.update(len(relative_path).to_bytes(4, "big"))
        digest.update(relative_path)
        digest.update(len(content).to_bytes(8, "big"))
        digest.update(content)
    return digest.hexdigest()


def artifact_hashes() -> dict[str, str]:
    return {
        "library_sources": sha256_tree((ROOT / "src").glob("*.rs")),
        "rust_benchmark": sha256_file(ROOT / "benches" / "comparison.rs"),
        "python_runner": sha256_file(ROOT / "benchmarks" / "compare.py"),
        "font": sha256_file(FONT_PATH),
        "cargo_manifest": sha256_file(ROOT / "Cargo.toml"),
        "cargo_lock": sha256_file(ROOT / "Cargo.lock"),
        "python_requirements": sha256_file(ROOT / "benchmarks" / "requirements.txt"),
    }


def build_report(
    rust_results: dict[str, dict[str, Any]],
    python_results: dict[str, dict[str, Any]],
    warmup: int,
    samples: int,
) -> dict[str, Any]:
    cases = []
    for case in CASES:
        rust = rust_results[case.name]
        python = python_results[case.name]
        expected_checksum = workload_checksum(frequencies(case.candidates))
        expected_mask_checksum = mask_checksum(make_mask(case))
        if rust["workload_checksum"] != expected_checksum:
            raise RuntimeError(f"Rust workload checksum mismatch for {case.name}")
        if python["workload_checksum"] != expected_checksum:
            raise RuntimeError(f"Python workload checksum mismatch for {case.name}")
        if rust["mask_checksum"] != expected_mask_checksum:
            raise RuntimeError(f"Rust mask checksum mismatch for {case.name}")
        if python["mask_checksum"] != expected_mask_checksum:
            raise RuntimeError(f"Python mask checksum mismatch for {case.name}")
        validate_render_metrics(case, rust["render_metrics"], python["render_metrics"])
        ratio = python["summary"]["median_ms"] / rust["summary"]["median_ms"]
        cases.append(
            {
                "case": asdict(case),
                "workload_checksum": expected_checksum,
                "mask_checksum": expected_mask_checksum,
                "rust": {
                    "times_ns": rust["times_ns"],
                    "placed_words": rust["placed_words"],
                    "render_metrics": rust["render_metrics"],
                    "summary": rust["summary"],
                },
                "python": python,
                "python_over_rust_median_ratio": ratio,
            }
        )

    git_status = command_output(["git", "status", "--short"])
    return {
        "schema_version": 2,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "git_revision": command_output(["git", "rev-parse", "HEAD"]),
        "git_worktree_dirty": bool(git_status),
        "machine": machine_metadata(),
        "versions": version_metadata(),
        "artifact_sha256": artifact_hashes(),
        "methodology": {
            "warmup_iterations": warmup,
            "measured_samples": samples,
            "endpoint": (
                "complete in-memory RGBA image from explicit frequencies; "
                "PNG encoding excluded"
            ),
            "rust_endpoint": "generate_detailed_from_frequencies",
            "python_endpoint": "generate_from_frequencies followed by to_image",
            "generator_reused": True,
            "python_rng_reset_outside_timing": True,
            "python_cyclic_gc_disabled_during_samples": True,
            "operations_per_sample": 1,
            "measurement_order": "first half Rust, all Python, second half Rust",
            "threads": 1,
            "font": str(FONT_PATH.relative_to(ROOT)),
            "font_step": 2,
            "margin": 2,
            "prefer_horizontal": 1.0,
            "case_specific_parameters": [
                "min_font_size",
                "max_font_size",
                "relative_scaling",
            ],
            "scale": 1,
            "seed": SEED,
            "output_mode": "RGBA",
        },
        "cases": cases,
    }


def validate_render_metrics(
    case: Case, rust: dict[str, Any], python: dict[str, Any]
) -> None:
    for implementation, metrics in (("Rust", rust), ("Python", python)):
        if metrics["placed_words"] <= 0 or metrics["ink_pixels"] <= 0:
            raise RuntimeError(f"{implementation} produced an empty {case.name} render")
        if metrics["placed_words"] > case.candidates:
            raise RuntimeError(
                f"{implementation} placed too many words for {case.name}"
            )

    if case.min_font_size != case.max_font_size:
        return
    for implementation, metrics in (("Rust", rust), ("Python", python)):
        expected_sum = case.candidates * case.min_font_size
        if metrics["placed_words"] != case.candidates:
            raise RuntimeError(
                f"{implementation} did not place every fixed-size word in {case.name}"
            )
        if (
            metrics["min_font_size"] != case.min_font_size
            or metrics["max_font_size"] != case.max_font_size
            or metrics["font_size_sum"] != expected_sum
        ):
            raise RuntimeError(
                f"{implementation} did not preserve the fixed font size in {case.name}"
            )


def print_summary(report: dict[str, Any]) -> None:
    print()
    print(
        "| case | candidates | placed (Rust/Python) | Rust median | Python median | Python/Rust |"
    )
    print("|---|---:|---:|---:|---:|---:|")
    for result in report["cases"]:
        case = result["case"]
        rust = result["rust"]["summary"]
        python = result["python"]["summary"]
        print(
            f"| {case['name']} | {case['candidates']} | "
            f"{format_number(rust['median_placed_words'])}/"
            f"{format_number(python['median_placed_words'])} | "
            f"{rust['median_ms']:.3f} ms | {python['median_ms']:.3f} ms | "
            f"{result['python_over_rust_median_ratio']:.2f}x |"
        )


def format_number(value: float) -> str:
    return str(int(value)) if float(value).is_integer() else f"{value:.1f}"


def main() -> None:
    args = parse_args()
    if not FONT_PATH.is_file():
        raise SystemExit(f"benchmark font is missing: {FONT_PATH}")
    output_path = args.output if args.output.is_absolute() else ROOT / args.output

    rust_samples_before = (args.samples + 1) // 2
    rust_samples_after = args.samples // 2
    rust_before = run_rust(args.warmup, rust_samples_before)
    python_results: dict[str, dict[str, Any]] = {}
    for case in CASES:
        print(f"Python: {case.name}", flush=True)
        python_results[case.name] = measure_python(case, args.warmup, args.samples)
    if rust_samples_after:
        rust_after = run_rust(args.warmup, rust_samples_after)
        rust_results = merge_rust_results(rust_before, rust_after)
    else:
        rust_results = rust_before

    report = build_report(rust_results, python_results, args.warmup, args.samples)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print_summary(report)
    try:
        displayed_output = output_path.relative_to(ROOT)
    except ValueError:
        displayed_output = output_path
    print(f"\nRaw results: {displayed_output}")


if __name__ == "__main__":
    main()
