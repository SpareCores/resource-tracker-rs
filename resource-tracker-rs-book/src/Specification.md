# Specification

Work In Progress

 - Assess feasibility and review open-source solutions landscape already relevant in this space with respect to the  Core and Extra Component functionality requirements below.
 - Develop rigorous Specification for Rust implementation of ResourceTracker CLI for Linux
 - Specification contains implementation plan.
 - Write tests of comparison against relevant open source functionality.
 - Proceed with implementation prototype.

### Core Components

 1. Linux implementation of resource tracker. Can rely on procfs if it makes sense.
 2. Support x86 and ARM.
 3. Configurable interval to track:

 | Item to Track          |
 |------------------------|
 | CPU(s)                 |
 | Memory                 |
 | GPU(s)                 |
 | VRAM                   |
 | Network traffic In/Out |
 | Disk Read/Write        |

 4. GPU requirement probably means cannot/should not be static linked.
 5. Command Line Linux tool
 6. Takes optional CLI params (e.g. metadata, such as what is the name of the job tracked). Sane defaults. Perhaps TOML config file.


### Extra Components

 7. Basic HTTP client to hit API Endpoints a) at the start of resource tracking, b) at the end, and c) every X minutes for heartbeat signal.
 8. Super lightweight (~without pkg dependency) implementation of S3 PUT object using AWS creds to a provided URI so that the package can batch "stream" resource utilization to a central place.
