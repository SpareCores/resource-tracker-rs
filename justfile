
set dotenv-load

help:
    @just --list

format:
	cargo fmt

build: format
    cargo build

## (cargo build --release) && upx target/release/resource-tracker
build_release:  format
	cargo build --release

install: build_release
	(mkdir -p ~/bin) && cp -p ./target/release/resource-tracker ~/bin && echo "resource-tracker is now installed in ~/bin"

run_only_show_key_names:
     target/release/resource-tracker --interval 3 | jq -r 'paths(scalars) as $p | "\($p | join(".")): \(getpath($p) | type)"'

# Build mdbook and then cargo doc bundled inside, then open the main page
document:
    (cd resource-tracker-rs-book && mdbook build) & cargo doc --no-deps --offline --target-dir resource-tracker-rs-book/book/cargo/ & wait
    xdg-open resource-tracker-rs-book/book/index.html &

test:
    env -u SENTINEL_API_TOKEN cargo test -- --test-threads=1

test_nocapture:
	env -u SENTINEL_API_TOKEN cargo test -- --test-threads=1 --nocapture



real_test1: build_release
	./target/release/resource-tracker  --format csv

real_test2: build_release
	./target/release/resource-tracker --format csv Rscript stress.r


real_test3: build_release
	./target/release/resource-tracker --format csv Rscript stress.r --cpu 4 --vm 1 --vm-bytes 12024M --timeout 63s


report_coverage:
    env -u SENTINEL_API_TOKEN cargo llvm-cov --bins --html --open -- --test-threads=1


## Audit dependencies for known vulnerabilities
audit:
	@command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit --locked
	cargo audit

outdated:
	@command -v cargo-outdated >/dev/null 2>&1 || cargo install cargo-outdated --locked
	cargo outdated


issue_20_test:
    cargo build --examples
    ./target/debug/resource-tracker --interval 1 -- ./target/debug/examples/repro_cpu_cutime_spike 2>&1 | grep --line-buffered '^{' | jq '{cores_process: .cpu.process_cores_used, cores_system: .cpu.utilization_pct}'

issue_20_test2: build_release
	TRACKER_QUIET=false sudo ./target/release/resource-tracker -o rt.log nice -n -20 python3 run_stressng_benchmarks.py



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
