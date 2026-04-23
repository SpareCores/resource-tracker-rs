# resource-tracker

A lightweight Linux resource & GPU tracker for batch processes.

## Rationale

This project was created to track the resource utilization of data science,
machine learning, AI, and other batch processes either used as a CLI wrapper, or
integrated into batch job orchestrators.

See the related [Resource Tracker Python implementation](https://github.com/SpareCores/resource-tracker)
for Python, R, and Metaflow-specific examples.

## CLI Usage

```bash
resource-tracker [FLAGS] -- <command> [args...]
```

The tracker will spawn `<command>`, monitor it, and exit when it exits.

By default, the tracked CPU, memory, GPU and other metrics are printed to stderr as JSON lines.
Both the output format and the output destination can be configured using flags or environment variables.

Optionally, the tracked process's metadata can be also provided using flags or environment variables.

See the [Usage Guide](resource-tracker-rs-book/src/Usage.md) for more details.

## Streaming

The `resource-tracker` also supports streaming resource usage data to a remote
location for central analysis, visualization, and future resource allocation
recommendations.

To get started, visit the <sentinel.sparecores.com> website to register a free
account, generate an API key, and use it to configure the `resource-tracker`
package via the `SENTINEL_API_KEY` environment variable.

Alternative API endpoints can be configured via the `SENTINEL_API_URL` environment variable.
