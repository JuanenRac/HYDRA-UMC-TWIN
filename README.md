<p align="center">
  <img src="images/HYDRA_UMC_BANNER.svg" alt="HYDRA-UMC-TWIN banner" width="100%">
</p>

# ♊ HYDRA-UMC-TWIN

<p align="center">🇺🇸 <b>English</b> | <a href="README_spa.md">🇪🇸 Español</a> | <a href="README_fra.md">🇫🇷 Français</a> | <a href="README_ita.md">🇮🇹 Italiano</a> | <a href="README_deu.md">🇩🇪 Deutsch</a> | <a href="README_zho.md">🇨🇳 简体中文</a> | <a href="README_jpn.md">🇯🇵 日本語</a></p>

### 🌐 Physics-Based Digital Twin & High-Fidelity Simulation Engine

<p align="left">
  <img src="https://img.shields.io/badge/Licencia-GPL%203.0-blue.svg" alt="GPL 3.0">
  <img src="https://img.shields.io/badge/Engine-Bevy%20%2F%20Rust-orange.svg" alt="Engine">
  <img src="https://img.shields.io/badge/Tech-MuJoCo%20%2F%20PhysX-blue.svg" alt="Physics">
  <img src="https://img.shields.io/badge/Feature-HIL%20Ready-green.svg" alt="HIL">
  <img src="https://img.shields.io/badge/Stage-Established%20v0-brightgreen.svg" alt="Established v0 stage">
</p>

---

## 1. 🛠️ TECHNICAL OVERVIEW

**HYDRA-UMC-TWIN** is the virtual heart of the ecosystem. It provides a high-fidelity, physics-based replica of the entire micro-factory, allowing for safe testing, training, and real-time monitoring of robotic swarms.

Built using Rust and the Bevy engine, it directly consumes URDF models from the EDITOR and emulates real-world physical properties like inertia, friction, and motor torque to ensure that "if it works in the Twin, it works on the floor."

### Key Features:
* 🧩 **Family Readiness Check (v0):** the real `family-status` subcommand reads each of the 3 real children's own `hydra-umc.project.json` and reports presence/version/maturity/role - honest for an integration hub that runs no engine itself yet. See "Honesty check" below.
* 🔒 **Real v0 - State-Sync Contract:** `family-sync` gates each child on a real, testable contract - minimum maturity (`functional`) and a maximum compatible major version - before ever treating it as sync-ready, refusing an immature or incompatible-version child with a real reason instead of syncing against an unverified state shape.
* 🌐 **Full Factory Simulation (planned):** replicates robots, tools, and the environment in a unified 3D space - depends on the real Bevy engine integration.
* ⚡ **Hardware-in-the-Loop (HIL) (planned):** connect Apps and Studios to the simulator as if it were a real controller.
* 📊 **Wear Prediction (planned):** estimates component lifespan based on simulated mechanical stress.
* 🛡️ **Safety Validation (planned):** test complex trajectories and collision avoidance before physical execution.

**Honesty check - what actually runs today:** bare invocation still prints identity/version/role, but there are now two real subcommands. `family-status [--workspace PATH]` reads `HYDRA-UMC-PHYSICS-REPLICA`/`HYDRA-UMC-HIL-BRIDGE`/`HYDRA-UMC-SYNTHETIC-DATA-GEN`'s own real manifests from a local checkout and reports what it honestly finds. `family-sync [--workspace PATH]` goes one step further: it runs each present child through a real state-sync contract (minimum maturity `functional`, maximum compatible major version) and reports `READY`, `REJECTED (immature)`, `REJECTED (incompatible version)`, or `MISSING` per child. No Bevy app, no rendering, no physics tick loop, no URDF scene loading, and no actual network sync transport exists yet - see [`CHANGELOG.md`](CHANGELOG.md) for exactly what shipped, and the Roadmap below for what's still ahead.

---

## 2. 🔄 TWIN ARCHITECTURE

```mermaid
flowchart TB
    URDF["URDF Models (EDITOR)"] --> TWIN["HYDRA-UMC-TWIN"]
    TWIN --> PHYS["PHYSICS-REPLICA (MuJoCo/PhysX)"]
    PHYS --> SYNC["HIL-BRIDGE (Command Sync)"]
    SYNC --> APP["Android / iOS App"]
    SYNC --> STUDIO["HYDRA-UMC-STUDIO"]
    TWIN --> DATA["SYNTHETIC-DATA-GEN"]
```

---

## 3. 🧱 ARCHITECTURE & DESIGN DECISIONS

* **Why this engine has no `hardware/`/`firmware/`/`os/` folders.** It is pure software with no board of its own, so source folders exist only when their implementation requires them.
* **Why `Cargo.toml` deliberately has no Bevy dependency yet.** Bevy is a heavy graphics engine - long compile times, needs a GPU/graphics toolchain that isn't always available. v0 only added `serde`/`serde_json` (for reading children's manifests) - real rendering work still waits for a real GPU/graphics toolchain to build against.
* **Why `docker-compose.yml` exists before its 3 children have Dockerfiles.** Deciding and documenting the integration contract (which service depends on which, what device/volume mounts each needs) now avoids that shape being invented ad hoc later, even though `docker compose up` can't fully succeed until each child publishes its own Dockerfile.
* **How this fits the rest of the ecosystem.** The integration parent of the Digital Twin & Simulation family - HYDRA-UMC-PHYSICS-REPLICA feeds it a real physics solver, HYDRA-UMC-HIL-BRIDGE lets real apps control it as if it were hardware, and HYDRA-UMC-SYNTHETIC-DATA-GEN renders training datasets through its own engine.
* **Why `family-status` reads each child's own manifest instead of a hand-maintained list.** `hydra-umc.project.json` is already the single source of truth the ecosystem's dashboard/updater trust - a second list here would drift the moment a child's real maturity changed and nobody remembered to update it.
* **Why a missing sibling checkout is a real, honest "not found" rather than a crash.** An integration hub genuinely cannot know whether a developer has all 3 children checked out locally - `manifest.rs` returns `None` for every real failure mode (missing repo, missing file, malformed JSON) so `family-status` can report it clearly instead of panicking.
* **Why `family-sync` gates on maturity AND a version ceiling, not just "is it there."** `family-status` already answers "is this child checked out and what does it claim" - but a checked-out, `scaffolding`-maturity child has no real state worth syncing yet, and a child that has bumped past this Twin's verified-compatible major version may have changed its own state shape in a way this Twin doesn't know about. Both are real reasons to refuse sync, distinct from "missing," so `contract.rs` checks and reports them separately rather than folding everything into one generic "not ready."
* **Why maturity is checked before version compatibility in `contract::assess()`.** An immature child's version number isn't a meaningful signal yet - checking maturity first means the reported rejection reason always names the most fundamental gate that actually failed, instead of a version mismatch masking a more basic "this child isn't real yet" problem.

---

## 📂 DIRECTORY STRUCTURE

Pure software engine with no hardware design of its own, so source folders
exist only when their implementation requires them; this project carries no
`hardware/`, `firmware/` or `os/` folders.

```text
HYDRA-UMC-TWIN/
├── src/
│   ├── manifest.rs       # Real, defensive reader for a sibling's own manifest
│   ├── family.rs         # Real family-readiness check + combined sync outcome
│   ├── contract.rs       # Real state-sync contract (maturity + version ceiling)
│   └── main.rs           # Entry point + real `family-status`/`family-sync` subcommands
├── docs/                # Documentation and physics tuning
├── build/               # Build notes/artifacts (cargo's own output lives in target/, gitignored)
├── images/              # Media and diagrams
├── scripts/             # Utility scripts
├── tools/
│   ├── build_test.py    # Non-versioning build/compile check
│   └── ci_validate.py   # Manifest/CHANGELOG/docs validation used by CI
├── Cargo.toml           # Package metadata, dependencies (serde/serde_json), odometer version
├── bump_version.py      # Odometer-style version bump (used by build.sh/.bat)
├── build.sh / build.bat # Bumps version, `cargo test`, then `cargo build --release`
├── build-test.sh / build-test.bat # Non-versioning build check (no CHANGELOG/version bump)
├── run.sh / run.bat     # Runs the compiled release binary (forwards arguments)
└── docker-compose.yml   # Integration blueprint for the 3 children below
```

---

## 🏗️ BUILD AND RUN

Requires the Rust toolchain (`cargo`/`rustc`, install via [rustup](https://rustup.rs)) and Python 3.10+ (only for `bump_version.py`).

```bash
# Linux / macOS
./build.sh   # odometer version bump, `cargo test` (21 tests), then `cargo build --release`
./run.sh     # runs target/release/hydra-umc-twin, prints name + version + role
```

```bat
:: Windows
build.bat
run.bat
```

`build.sh`/`build.bat` bump this project's own `Cargo.toml` version following the ecosystem's "odometer" rule (PATCH+1, carrying into MINOR past 9), run the real test suite, then build a release binary.

The real `family-status` subcommand checks the actual local checkout:

```bash
./run.sh family-status
./run.sh family-status --workspace /path/to/some/other/checkout

# Windows
run.bat family-status
```

```text
Digital Twin family status (workspace: /path/to/GitHub):
  HYDRA-UMC-PHYSICS-REPLICA: v0.0.2, maturity=functional, role=library
  HYDRA-UMC-HIL-BRIDGE: v0.0.1, maturity=scaffolding, role=service
  HYDRA-UMC-SYNTHETIC-DATA-GEN: v0.0.4, maturity=functional, role=tool

All 3 children present.
```

Defaults to this repo's own parent directory - the real sibling-checkout layout this ecosystem already uses. Exits `1` if any real child is missing.

The real `family-sync` subcommand goes further - it also checks the real state-sync contract (minimum maturity, maximum compatible major version) against each child that is present:

```bash
./run.sh family-sync --workspace /path/to/some/checkout
```

```text
Digital Twin family sync contract (workspace: /path/to/some/checkout):
  HYDRA-UMC-PHYSICS-REPLICA: READY (v0.0.3, maturity=functional)
  HYDRA-UMC-HIL-BRIDGE: REJECTED (incompatible version) - HYDRA-UMC-HIL-BRIDGE reports major version 1 - this Twin's sync contract is only verified up to major 0 (incompatible simulator version)
  HYDRA-UMC-SYNTHETIC-DATA-GEN: MISSING (not checked out)

Not every child is sync-ready - see the lines above.
```

Exits `0` only if every expected child is `READY`; `1` for any `MISSING`/`REJECTED` child.

**Important:** `Cargo.toml` deliberately has **no Bevy dependency yet**. Bevy is a heavy graphics engine (long compile times, needs a GPU/graphics toolchain that isn't always available); v0 only added `serde`/`serde_json` for reading manifests. The real `bevy` dependency (plus a physics backend and the gRPC/WebSocket client for HIL-BRIDGE) is added when real rendering/engine work starts.

### Integrating the 3 children (`docker-compose.yml`)

As the integration parent, `docker-compose.yml` documents how this engine composes its 3 children into one stack: **PHYSICS-REPLICA** (solver, called every physics tick), **HIL-BRIDGE** (real-vs-virtual command sync), and **SYNTHETIC-DATA-GEN** (offline batch dataset export). None of the 4 projects has a `Dockerfile` yet at skeleton stage, so `docker compose up` is not runnable today; the file is the confirmed topology/ports/dependency-graph reference for future Dockerfiles.

---

## 🚀 ROADMAP
* **Phase 1:** Digital Twin synchronization with real-time hardware telemetry and sub-10ms latency.
* **Phase 2:** Physics Replica integration with industrial-grade simulators (Isaac Sim) and deformable body support.
* **Phase 3:** Node Healing automated recovery patterns for decentralized failover and early sensor degradation detection.
* **Phase 4:** Photorealistic rendering for synthetic data generation and HIL Bridge support for full-scale vehicle-in-the-loop.

---

## 🔗 Related Projects

This project is part of a larger robotics ecosystem by the same author (JuanenRac / Electro Hobby 3D), spanning firmware, control software, AI nodes, and fleet tooling. Worth knowing about, since a request might actually be about one of these rather than this repository.

### Family

**Parent:** none — this project is itself the integration parent of the Digital Twin & Simulation family.

**Children:**
- **[HYDRA-UMC-PHYSICS-REPLICA](https://github.com/JuanenRac/HYDRA-UMC-PHYSICS-REPLICA)** — the rigid-body/contact simulation feeding this renderer.
- **[HYDRA-UMC-HIL-BRIDGE](https://github.com/JuanenRac/HYDRA-UMC-HIL-BRIDGE)** — the hardware-in-the-loop link this twin drives real I/O through.
- **[HYDRA-UMC-SYNTHETIC-DATA-GEN](https://github.com/JuanenRac/HYDRA-UMC-SYNTHETIC-DATA-GEN)** — renders training datasets through this twin's own engine.

### Directly Related (outside the family)

- **[HYDRA-UMC-EDITOR-URDF](https://github.com/JuanenRac/HYDRA-UMC-EDITOR-URDF)** — consumes the URDF models authored here.
- **[HYDRA-UMC-SUITE](https://github.com/JuanenRac/HYDRA-UMC-SUITE)** — controls this twin as if it were real hardware, via HIL-BRIDGE.
- **[HYDRA-UMC-ANDROID-CONTROL](https://github.com/JuanenRac/HYDRA-UMC-ANDROID-CONTROL)** — controls this twin as if it were real hardware, via HIL-BRIDGE.
- **[HYDRA-UMC-IOS-CONTROL](https://github.com/JuanenRac/HYDRA-UMC-IOS-CONTROL)** — controls this twin as if it were real hardware, via HIL-BRIDGE.

### Rest of the Ecosystem

**HYDRA-UMC platform** — the multi-robot micro-factory cell
- **[HYDRA-UMC](https://github.com/JuanenRac/HYDRA-UMC)** — the CM5 + STM32H745 motherboard orchestrating up to 8 robot arms.
- **[HYDRA-UMC-SERVER](https://github.com/JuanenRac/HYDRA-UMC-SERVER)** — the Express/WebSocket backend every control client talks to.
- **[HYDRA-UMC-STUDIO](https://github.com/JuanenRac/HYDRA-UMC-STUDIO)** — web-based control dashboard, multi-robot 3D visualization.
- **[HYDRA-UMC-ANDROID-CONTROL](https://github.com/JuanenRac/HYDRA-UMC-ANDROID-CONTROL)** — Android control app over Wi-Fi/Bluetooth.
- **[HYDRA-UMC-IOS-CONTROL](https://github.com/JuanenRac/HYDRA-UMC-IOS-CONTROL)** — iOS/iPadOS control app built in Flutter.
- **[HYDRA-UMC-SUITE](https://github.com/JuanenRac/HYDRA-UMC-SUITE)** — desktop swarm command center (Python/PySide6).
- **[HYDRA-UMC-EDITOR-URDF](https://github.com/JuanenRac/HYDRA-UMC-EDITOR-URDF)** — desktop URDF model editor for the robot catalog.
- **[HYDRA-UMC-DSI](https://github.com/JuanenRac/HYDRA-UMC-DSI)** — native touch UI for the onboard DSI touchscreen.

**URTC platform** — the tool head controller every HYDRA-UMC robot arm carries
- **[URTC](https://github.com/JuanenRac/URTC)** — CAN bus tool head controller, 25 tool profiles.
- **[URTC-FLASHER](https://github.com/JuanenRac/URTC-FLASHER)** — desktop CAN-OTA + SWD/JTAG flashing tool.
- **[URTC-TESTER](https://github.com/JuanenRac/URTC-TESTER)** — desktop live CAN-bus diagnostic tool.
- **[URTC-WEB-STUDIO](https://github.com/JuanenRac/URTC-WEB-STUDIO)** — browser-based alternative via Web Serial API.

**🎥 Vision AI Node (Hailo-8)**
- [HYDRA-UMC-VISION-NODE](https://github.com/JuanenRac/HYDRA-UMC-VISION-NODE)
- [HYDRA-UMC-VISION-STREAMER](https://github.com/JuanenRac/HYDRA-UMC-VISION-STREAMER)
- [HYDRA-UMC-DETECTION-HEF](https://github.com/JuanenRac/HYDRA-UMC-DETECTION-HEF)
- [HYDRA-UMC-SAFETY-ZONES](https://github.com/JuanenRac/HYDRA-UMC-SAFETY-ZONES)
- [HYDRA-UMC-VISUAL-SERVOING-API](https://github.com/JuanenRac/HYDRA-UMC-VISUAL-SERVOING-API)

**🧠 Cognitive AI Node (Hailo-10)**
- [HYDRA-UMC-COGNITIVE-NODE](https://github.com/JuanenRac/HYDRA-UMC-COGNITIVE-NODE)
- [HYDRA-UMC-VLA-ENGINE](https://github.com/JuanenRac/HYDRA-UMC-VLA-ENGINE)
- [HYDRA-UMC-VOICE-UI](https://github.com/JuanenRac/HYDRA-UMC-VOICE-UI)
- [HYDRA-UMC-SEMANTIC-PLANNER](https://github.com/JuanenRac/HYDRA-UMC-SEMANTIC-PLANNER)
- [HYDRA-UMC-DOCS-QA](https://github.com/JuanenRac/HYDRA-UMC-DOCS-QA)

**🐝 Orchestration & Swarm**
- [HYDRA-UMC-ORCHESTRATOR](https://github.com/JuanenRac/HYDRA-UMC-ORCHESTRATOR)
- [HYDRA-UMC-SWARM-SYNC](https://github.com/JuanenRac/HYDRA-UMC-SWARM-SYNC)
- [HYDRA-UMC-PATH-PLANNER-3D](https://github.com/JuanenRac/HYDRA-UMC-PATH-PLANNER-3D)
- [HYDRA-UMC-JOB-DISPATCHER](https://github.com/JuanenRac/HYDRA-UMC-JOB-DISPATCHER)
- [HYDRA-UMC-NODE-HEALING](https://github.com/JuanenRac/HYDRA-UMC-NODE-HEALING)

**📊 Data & Analytics**
- [HYDRA-UMC-DATALAKE](https://github.com/JuanenRac/HYDRA-UMC-DATALAKE)
- [HYDRA-UMC-TELEMETRY-COLLECTOR](https://github.com/JuanenRac/HYDRA-UMC-TELEMETRY-COLLECTOR)
- [HYDRA-UMC-ANOMALY-DETECTOR](https://github.com/JuanenRac/HYDRA-UMC-ANOMALY-DETECTOR)
- [HYDRA-UMC-PRODUCTION-REPORTS](https://github.com/JuanenRac/HYDRA-UMC-PRODUCTION-REPORTS)

**🏭 Industrial Gateway**
- [HYDRA-UMC-GATEWAY-INDUSTRIAL](https://github.com/JuanenRac/HYDRA-UMC-GATEWAY-INDUSTRIAL)
- [HYDRA-UMC-OPCUA-SERVER](https://github.com/JuanenRac/HYDRA-UMC-OPCUA-SERVER)
- [HYDRA-UMC-MQTT-BROKER](https://github.com/JuanenRac/HYDRA-UMC-MQTT-BROKER)
- [HYDRA-UMC-MTCONNECT-ADAPTER](https://github.com/JuanenRac/HYDRA-UMC-MTCONNECT-ADAPTER)

**🛠️ Complementary Tools**
- [URTC-SMART-RACK](https://github.com/JuanenRac/URTC-SMART-RACK)
- [URTC-VISION-TOOL](https://github.com/JuanenRac/URTC-VISION-TOOL)
- [HYDRA-UMC-WATCH](https://github.com/JuanenRac/HYDRA-UMC-WATCH)
- [HYDRA-UMC-TOOL-CLI](https://github.com/JuanenRac/HYDRA-UMC-TOOL-CLI)
- [HYDRA-UMC-DASHBOARD-AI](https://github.com/JuanenRac/HYDRA-UMC-DASHBOARD-AI)


## 👤 AUTHOR
**JuanenRac** (Electro Hobby 3D)
📧 electrohobby3d@gmail.com

## 📜 LICENSE
GPL-3.0 - See LICENSE for details.

## 🛠️ BUILD & RUN

Use the non-versioning build check before a release build:

| Action | Windows | Linux / macOS |
|---|---|---|
| Build check (no version or CHANGELOG change) | `build-test.bat` | `./build-test.sh` |
| Run / development (when provided) | `run*.bat` or `dev*.bat` | `./run*.sh` or `./dev*.sh` |

`build-test.bat` and `build-test.sh` compile or validate the project stack without incrementing `hydra-umc.project.json` or modifying `CHANGELOG.md`. They may create normal compiler output only. Existing `build*.bat`, `build*.sh`, `run*` and `dev*` scripts retain their project-specific, versioned or runtime behavior; use them when that behavior is required.