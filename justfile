
help:
    @just --list


build:
    cargo build

build_release:
    cargo build --release

# GPU machine: requires AMD driver (libdrm) + NVIDIA driver (libnvidia-ml.so) at runtime.
# Both libraries are loaded dynamically; the binary degrades gracefully if either is absent.
build_gpu:
    cargo build

build_release_gpu:
    cargo build --release

run_only_show_key_names:
     target/release/resource-tracker-rs --interval 3 | jq -r 'paths(scalars) as $p | "\($p | join(".")): \(getpath($p) | type)"'

document:
    cd resource-tracker-rs-book && mdbook build && open book/index.html && cd -
    cargo doc --open

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

