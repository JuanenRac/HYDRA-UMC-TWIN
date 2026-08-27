# Contributing to HYDRA-UMC-TWIN 🦾

We welcome contributions to the high-fidelity digital twin of the HYDRA-UMC platform.

## Technology Stack
- **Language**: Rust 1.80+.
- **Engine**: Bevy (ECS-based).
- **Physics**: MuJoCo / NVIDIA PhysX.
- **Rendering**: WGPU, glTF 2.0.

## Guidelines
1. **ECS Best Practices**: Follow Bevy's Entity Component System patterns. Use plugins for modular feature development.
2. **Physics Fidelity**: All mechanical changes must be validated against real-world motor torque and inertia curves.
3. **Asset Management**: Ensure all 3D assets are optimized for real-time rendering and follow the project's glTF naming conventions.
4. **HIL Sync**: Validate that any change to the state engine does not introduce more than 1ms of synchronization jitter in the HIL bridge.
