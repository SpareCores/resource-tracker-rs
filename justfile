
help:
    @just --list


build:
    cargo build

build_release:
    (cargo build --release) && upx target/release/resource-tracker-rs

# GPU machine: requires AMD driver (libdrm) + NVIDIA driver (libnvidia-ml.so) at runtime.
# Both libraries are loaded dynamically; the binary degrades gracefully if either is absent.
build_gpu:
    cargo build

build_release_gpu:
    cargo build --release

run_only_show_key_names:
     target/release/resource-tracker-rs --interval 3 | jq -r 'paths(scalars) as $p | "\($p | join(".")): \(getpath($p) | type)"'

# Build cargo doc and mdbook in parallel, then open both in the browser
# without either open blocking the other.
document:
    (cd resource-tracker-rs-book && mdbook build) & cargo doc & wait
    xdg-open resource-tracker-rs-book/book/index.html &
    xdg-open target/doc/resource_tracker_rs/index.html &

test:
    cargo test


## Stub for possisble future use:
## # Install Python resource-tracker via uv
## bench_setup:
##     cd benchmarks && uv sync
## 
## # Run both trackers simultaneously for 60 s, CSV output on both sides
## bench_run:
##     mkdir -p benchmarks/results
##     bash benchmarks/run_rust.sh &
##     cd benchmarks && uv run python run_python.py
##     wait
## 
## # Compare outputs and print diff table
## bench_compare:
##     cd benchmarks && uv run python compare.py
## 
## # Full pipeline
## benchmark: bench_setup bench_run bench_compare
## 

