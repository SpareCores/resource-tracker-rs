This is a placeholder for the `sparecores-resource-tracker-rs` project.

# Initial Specification/Ideation

The [`resource-tracker` Python package](https://github.com/SpareCores/resource-tracker)
was brought to life in 2025 to have a way to track the resources used by long-running
DS/ML/AI jobs in the cloud, and recommend better cloud resource allocations.
This was started as an experimentation and resulted in the following features:

- Supports Linux, macOS, and Windows. No dependencies on Linux, required
  `psutil` on other operating systems.
- Tracks CPU, memory, NVIDIA GPU and VRAM (even at the process level), disk
  usage, network usage at the system and process level.
- Monitoring happens at a configurable interval (defaults to 1 second), and
  collects metrics to local (temp) CSV files.
- Performance is unnoticeable at 1-sec frequency, but cannot go much lower
  without significant performance overhead.
- Computes aggregated statistics on the metrics (e.g. average and peak values).
- Recommends optimal cloud resource allocations based on the metrics.
- Recommends best-priced cloud servers for the given workload.
- Renders a local HTML report with all the metrics and recommendations.
- Has an R package wrapper for the same functionality.
- Integrates well with Metaflow.

While it worked well for Python and R, we also wanted a standalone tool that can
be better used as a CLI wrapper to track any processes in any environment, and
eventually integrate back in the existing Python and R packages. The overall
goal is to have a lightweight binary, compiled cross-platform, that can

- Track a wide range of resource utilization metrics locally -- including CPU,
  memory, GPU and VRAM, disk usage, network usage.
- Optionally stream these metrics to a remote server for centralized analysis,
  visualization, and further optimization.

  This allows us not to embed any complex logic in the binary, and just focus on
  data collection and delivery, so that am accompained free/commercial service
  can deliver the centeralized visibility, recommendations, automation and
  optimization -- while keeping most of the ecosystem open-source and open to
  extend with other tools and services.

# Data Collection

## Discovery Tools

What worked great in the Python implementation was the ability to discover the

- Most important specs of the host machine, such as CPU cores count, memory amount etc.
- Cloud environment of the server (when available), such as vendor, region, instance type.

These limited tools are implemented at

- https://github.com/SpareCores/resource-tracker/blob/main/src/resource_tracker/server_info.py
- https://github.com/SpareCores/resource-tracker/blob/main/src/resource_tracker/cloud_info.py

We are sure the hardware discovery could be improved further, and we aim to
collect at least the following (all prefixed with `host_` in the data ingestion
endpoint):

- host_id (text): Unique identifier of the host machine, such as AWS EC2 instance ID or the server S/N.
- host_name (text): Hostname of the machine.
- host_ ip (text): IP address of the machine.
- host_allocation (enum): If the server is dedicated to the monitored process, or shared with other processes.
- host_vcpus (int): Number of logical virtual CPU cores.
- host_cpu_model (text): Model of the CPU (e.g. from `lscpu` output).
- host_memory_mib (int): Amount of memory in MiB.
- host_gpu_model (text): Model of the GPU (e.g. from `nvidia-smi` output).
- host_gpu_count (int): Number of GPUs.
- host_gpu_vram_mib (int): Amount of VRAM in MiB.
- host_storage_gb (float): Amount of storage in GB.

All these fields are optional, and only collected when available. Users should
be able to suppress any sensitive fields, such as the host IP address.

The cloud discovery is implemented via probing the Metadata server endpoints of
the supported cloud providers. We should try to get information about the
following fields (all using the `cloud_` prefix in the data ingestion endpoint):

- cloud_vendor_id (text): The cloud provider's id, mapped to the Spare Cores
  Navigator's vendor table reference (e.g. `aws`).
- cloud_account_id (text): The cloud account id.
- cloud_region_id (text): The cloud region id, mapped to the Spare Cores
  Navigator's region table reference (e.g. `us-east-1`).
- cloud_zone_id (text): The cloud zone id, mapped to the Spare Cores Navigator's
  zone table reference (e.g. `us-east-1a`).
- cloud_instance_type (text): The cloud instance type, mapped to the Spare Cores
  Navigator's server table's `api_reference` field (e.g. `t3a.nano`).

Find the Spare Cores Navigator's vendor, region, zone and server tables at
https://github.com/SpareCores/sc-data-dumps/tree/main/data and schemas described at
https://dbdocs.io/spare-cores/sc-crawler.

## Metrics to Track

The data ingestion endpoint is rather liberal and any arbitrary metric can be
tracked. The only restriction is that the submitted data needs to be a CSV file
with at least one column named `timestamp`, which should be UNIX timestamp in
seconds.

All other columns are treated as metrics. We recommend storing machine-wide
metrics prefixed with `system_` and the process-level metrics prefixed with
`process_`. If distinguishing between machine-wide and process-level metrics is
not feasible, metrics can be submitted without any prefix.

Recommended column names for commonly tracked process-level metrics that are
taken into consideration in the backend:

- children: The number of child processes.
- utime: The total user+nice mode CPU time in seconds.
- stime: The total system mode CPU time in seconds.
- cpu_usage: The current CPU usage between 0 and number of CPUs.
- memory_mib: Current memory usage in MiB. Preferably PSS (Proportional Set
  Size) on Linux, fall back to RSS (Resident Set Size).
- disk_read_bytes: The total number of bytes read from disk.
- disk_write_bytes: The total number of bytes written to disk.
- gpu_usage: The current GPU utilization between 0 and GPU count.
- gpu_vram_mib: The current GPU memory used in MiB.
- gpu_utilized: The number of GPUs with utilization > 0.

Recommended column names for commonly tracked machine-wide metrics that are
taken into consideration in the backend:

- processes: The number of running processes.
- utime: The total user+nice mode CPU time in seconds.
- stime: The total system mode CPU time in seconds.
- cpu_usage: The current CPU usage between 0 and number of CPUs.
- memory_free_mib: The amount of free memory in MiB.
- memory_used_mib: The amount of used memory in MiB.
- memory_buffers_mib: The amount of memory used for buffers in MiB.
- memory_cached_mib: The amount of memory used for caching in MiB.
- memory_active_mib: The amount of memory used for active pages in MiB.
- memory_inactive_mib: The amount of memory used for inactive pages in MiB.
- disk_read_bytes: The total number of bytes read from all disks.
- disk_write_bytes: The total number of bytes written to all disks.
- disk_space_total_gb: The total disk space in GB.
- disk_space_used_gb: The used disk space in GB.
- disk_space_free_gb: The free disk space in GB.
- net_recv_bytes: The total number of bytes received over network.
- net_sent_bytes: The total number of bytes sent over network.
- gpu_usage: The current GPU utilization between 0 and GPU count.
- gpu_vram_mib: The current GPU memory used in MiB.
- gpu_utilized: The number of GPUs with utilization > 0.

No other metrics are officially supported by the backend at the moment, but the
user can submit any arbitrary values (even strings!) for future use.

## Metadata

We also want to support collecting the following metadata about the monitored process:

- pid (int): The process ID.
- container_image (text): The container image, including optional tag.
- command (json): JSON array of the command and its arguments.
- env (text): The environment (e.g. dev or prod).
- language (text): The language of the process (e.g. python or r).
- orchestrator (text): The orchestrator of the process (e.g. metaflow).
- executor (text): The executor of the process (e.g. k8s).
- team (text): The team of the process.
- project_name (text): The project name of the process.
- job_name (text): The job name of the process (e.g. flow in metaflow, workflow in flyte).
- stage_name (text): The stage name of the process (e.g. step in metaflow, node in flyte).
- task_name (text): The task name of the process (e.g. task both in metaflow and flyte).
- external_run_id (text): The external run id of the process (e.g. Jenkins build
  number -- internal to the orchestrator).

Most (if not all: except for the `command`) of these fields are to be provided
voluntarily and manually by the user (or job orchestrator) and should be optional.
Privacy and security concerns are addressed in the public service's legal docs.

The user should be also able to provide any ad-hoc key-value pairs (tags) for
tracking purposes.

## Status

The data ingestion endpoint automatically captures the start and end time of the
process, and calculates the duration in seconds. It also captures user and
organization information based on the user's credentials. Once a job is
finished, statistics and recommendations are calculated and stored in a
database, made available to the user via a web interface, API, and potentially
via the CLI tool as well in the future.

But the CLI tool need to collect the following fields and pass to the data
ingestion endpoint:

- exit_code (int): The exit code of the process.
- run_status (enum): The status of the run (e.g. success, failure, etc).

# Data Streaming

To authenticate with the data ingestion API endpoint, the Resource Tracker needs
to use a long-lived API token set by the user in the `SENTINEL_API_TOKEN`
environment variable. This needs to be passed as the `Authorization` header with
the value `Bearer <token>`.

At the start of the Resource Tracker, hit the data ingestion endpoint to
register the start of a `Run` along with the following optional parameters:

- metadata (e.g. `project_name` etc.)
- server and cloud discovery information (e.g. number of CPUs and/or actual instance type)

The response contains:

- `run_id` that should be stored until the end of the run as all future API
   calls will need to reference that.
- `upload_uri_prefix`: An S3 URI prefix to upload the metrics to.
- `upload_credentials`: The temporary AWS STS session credentials for the upload
  authentication, including an expiry date.

Then the Resource Tracker should start a background thread (or similar solution)
to upload collected metrics in batches (e.g. every 1 minute) as new objects
under the `upload_uri_prefix` as gzipped CSV files. The Resource Tracker should
also keep track of the uploaded URIs.

When the temporary upload credentials expire, the Resource Tracker should hit the
data ingestion endpoint to refresh the credentials.

When the tracked process finishes, the Resource Tracker should hit the data
ingestion endpoint to register the end of the run. This takes

- The `run_id`,
- The status of the run (e.g. success, failure, etc.) along with an optional
  `exit_code` as described above,
- And either the list of the uploaded URIs as `data_uris` along with
  `data_source` set to `s3`, or if no S3 uploads happened yet (e.g. short
  duration run), then the CSV file as `data_csv` along with `data_source` set to
  `local`.

The endpoint will process the data in synchronous manner, and return statistics.

## More Details

Find the data ingestion API endpoints docs at ... (TBD), including the data
contracts and API references.
