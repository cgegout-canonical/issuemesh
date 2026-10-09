# IssueMesh Snap Packaging Guide

This guide covers building, installing, testing, and distributing **IssueMesh** as a strictly confined Snap package for Ubuntu and Linux distributions using `core24`.

---

## 1. Prerequisites

Ensure Snapcraft and LXD are installed and initialized on your host system:

```bash
# Install Snapcraft and LXD
sudo snap install snapcraft --classic
sudo snap install lxd

# Initialize LXD (if not previously configured)
sudo lxd init --auto
```

---

## 2. Building the Snap

IssueMesh builds in a clean, isolated LXD container running Ubuntu 24.04 LTS (`core24`).

Run the pack command from the repository root:

```bash
export SNAPCRAFT_BUILD_INFO=1
snapcraft pack
```

*Note: Never use `--destructive-mode`. Building inside the LXD container guarantees reproducible binaries without host contamination.*

Upon completion, a `.snap` file will be generated in the project root:
```text
issuemesh_0.1.0_amd64.snap
```

---

## 3. Installation & Testing

### Initial Testing (Devmode)
To verify packaging and binary startup without security confinement:
```bash
sudo snap install --devmode issuemesh_0.1.0_amd64.snap
issuemesh --version
```

### Strict Confinement Installation
To test under real AppArmor and SecComp confinement:
```bash
sudo snap install --dangerous issuemesh_0.1.0_amd64.snap
```

---

## 4. Interfaces & Permissions

IssueMesh requests three core interfaces:

| Plug | Type | Auto-Connected | Description |
|:---|:---|:---:|:---|
| `network` | AppArmor / Network | Yes | Outbound HTTP/HTTPS requests to GitHub, Jira, Launchpad, and LLM endpoints |
| `network-bind` | AppArmor / Network | Yes | Binds local port (default 8765) to serve the Web UI and REST API |
| `home` | AppArmor / Filesystem | Yes | Optional access to home directory files |

If manual reconnection is ever required:
```bash
sudo snap connect issuemesh:network
sudo snap connect issuemesh:network-bind
sudo snap connect issuemesh:home
```

---

## 5. Storage & Layout

Under Snap strict confinement:
- **Shared Data & DB**: `$SNAP_USER_COMMON` (e.g. `~/snap/issuemesh/common/`) is used automatically for the SQLite database (`issuemesh.db`), search run history, and cache.
- **Portability**: IssueMesh contains zero external dynamic dependencies on `libssl` or `libsqlite3` (pure Rustls and bundled SQLite engine).

---

## 6. Running IssueMesh as a Snap

Start the interactive Web UI and server:
```bash
issuemesh
```

Run on a custom port:
```bash
issuemesh --port 9000
```

Specify a custom data directory:
```bash
issuemesh --data-dir $SNAP_USER_COMMON/custom_data
```

---

## 7. Troubleshooting & Diagnostics

- **Shell into snap environment**:
  ```bash
  snap run --shell issuemesh
  ```
- **Inspect AppArmor / SecComp security denials**:
  ```bash
  sudo snappy-debug
  # or
  journalctl -e --no-pager | grep -i apparmor
  ```
