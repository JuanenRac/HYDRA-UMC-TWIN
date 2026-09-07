# Security Policy 🔒 (HYDRA-UMC-TWIN)

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.x.x  | ✅ Yes             |

## Reporting a Vulnerability

**CRITICAL: Do not report safety-critical vulnerabilities through public GitHub issues.**

In a digital twin used for safety validation, a security flaw can lead to false confidence in dangerous paths. If you discover a vulnerability affecting the **physics solver accuracy**, **HIL command interception**, or **3D asset poisoning**:

1. **Email**: Send a detailed report to `electrohobby3d@gmail.com`.
2. **Impact**: Describe if the bug allows bypassing collision detection, spoofing hardware feedback, or causing remote crashes via malicious URDF models.
3. **Response**: Initial acknowledgment within 48 hours.

We follow a coordinated disclosure policy to ensure hardware safety before public release.
