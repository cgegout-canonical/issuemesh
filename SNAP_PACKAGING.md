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

### Option A: Direct Build (Fast & Native)

Builds the optimized release binary, stages the full web frontend, SQL datastore schema, desktop integration, and packs the `.snap` directly using the host `snap pack`:

```bash
./scripts/build_snap.sh
```

Upon completion, `issuemesh_0.2.8_amd64.snap` is generated in the project root.

### Option B: Isolated LXD Container Build (`snapcraft pack`)

IssueMesh can also build in a clean, isolated LXD container running Ubuntu 24.04 LTS (`core24`):

```bash
export SNAPCRAFT_BUILD_INFO=1
snapcraft pack
```

---

## 3. Installation & Testing

### Strict Confinement Installation (Recommended)
```bash
sudo snap install --dangerous issuemesh_0.2.8_amd64.snap
```

### Initial Testing (Devmode)
```bash
sudo snap install --devmode issuemesh_0.2.8_amd64.snap
issuemesh --version
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
