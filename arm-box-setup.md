# ARM box setup

For testing purposes, spin up an ARM architecture linux machine and set things up for testing the project:

```bash
sudo apt-get update && sudo apt-get install -y build-essential gcc and
sudo apt-get install -y pkg-config libssl-dev
echo '. "$HOME/.cargo/env"' >> ~/.bashrc
cargo install just
git clone https://github.com/SpareCores/resource-tracker-rs.git
cd resource_tracker_rs
```
