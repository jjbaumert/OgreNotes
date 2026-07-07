# Security Policy

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
pull requests, or discussions.**

Instead, use GitHub's private vulnerability reporting:

1. Go to the repository's **Security** tab.
2. Click **Report a vulnerability** (Private Vulnerability Reporting).
3. Provide a description, reproduction steps, affected version/commit, and
   impact assessment.

> Maintainer: enable Private Vulnerability Reporting under
> **Settings → Code security and analysis** before publishing, or replace this
> section with a dedicated security contact email if you prefer.

## What to expect

OgreNotes is a personal project maintained on a best-effort basis. There is no
SLA. That said, security reports are taken seriously:

- Acknowledgement of your report as soon as the maintainer is able.
- An assessment of severity and, where warranted, a fix.
- Credit for the report if you would like it (let us know).

## Scope

This project handles authentication (OAuth2/JWT), document sharing and
permissions, real-time collaboration over WebSocket, and an LLM-backed
assistant. Reports touching any of the following are especially valuable:

- Authentication / session handling / JWT verification
- Authorization and document/folder access control (sharing, link sharing)
- Prompt injection or data exfiltration via the AI assistant
- WebSocket authentication and message authorization
- Admin endpoints and privilege escalation

## Out of scope

- Issues in dependencies with no OgreNotes-specific impact (report upstream;
  `deny.toml` tracks known advisories with documented rationale).
- Findings that require a compromised host or physical access.
- Missing hardening headers on a purely local development deployment.
