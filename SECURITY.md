# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue: use
[Report a vulnerability](https://github.com/studio-hills/meshvault/security/advisories/new)
on the repository's Security tab. Say what is affected, how to reproduce it,
and what an attacker could do with it. Once a fix is released, the advisory is published with credit to you, unless
you'd rather not be named.

## Supported versions

Fixes go into the latest release only.

## What counts

Meshvault handles a few things where a bug could hurt its users:

- **Credentials.** The Open Cloud API key or `.ROBLOSECURITY` cookie is kept
  in Windows Credential Manager and should only ever be sent to Roblox. Any way
  it could be written to disk, logged, shown, or sent elsewhere is a
  vulnerability.
- **The Studio link.** Meshvault listens on `localhost:34875` for Roblox
  Studio. It accepts connections from this machine only and refuses exports
  from web pages. It has no other authentication, so any program on this
  machine can send it an export; that is by design. A way for another machine
  or a website to reach it is a vulnerability.
- **Untrusted files.** Meshes, images and exports come from other people.
  A file that makes Meshvault crash, hang, use unbounded memory, or write
  outside its workspace is worth reporting; one that leads to code execution
  is serious.

Outside the scope: what Roblox lets an account download (Meshvault only uses
Roblox's own asset delivery with your credentials), and problems that need an
attacker who already controls your Windows account.
