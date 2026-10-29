# Security

## Reporting a problem

Please report vulnerabilities privately through GitHub: **Security → Report a vulnerability** on this repository. Do not open a public issue for something exploitable. Include the version (`imc://version`), what you did and what happened.

## What the browser defends against

- **Hostile web pages**: Chromium's sandbox (renderers at Untrusted integrity), site isolation, deny-by-default permissions, HTTPS-first, native tracker blocking.
- **Pages impersonating the browser**: the browser's own UI page can only navigate to itself and has a content security policy; internal `imc://` pages cannot be loaded or read by websites; every message a page can print to the console that the browser listens for is validated (see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)); pop-ups may only open web addresses.
- **Credential theft**: passwords and cards are encrypted with Windows DPAPI; fill is manual, HTTPS-only, and only on the exact site a login was saved for.
- **Tampered updates**: Ed25519-signed manifests, SHA-256 and size checks, rollback protection.
- **Malicious extension packages**: path-traversal and zip-bomb limits, https-only downloads, extension installs from store pages only on the store's own pages.
- **DLL planting**: DLLs load only from the system folder and the app's folder; helper programs (curl, cmd, explorer) are run by full system path.

## What it does not defend against

- Malware already running as your Windows user (it could read your profile folder or the AI-control pipe).
- Extensions you install: an extension with broad host permissions can see the pages it runs on. Install only what you trust.
- Passwords and history on disk are only as private as your Windows account (passwords and cards are encrypted with DPAPI; history, bookmarks and session are not).

## Known limits

- Chrome Web Store and Firefox add-on packages are fetched over HTTPS; their package signatures are not separately verified.
- Test-only hooks (`IMC_SELFTEST*`) are compiled out of release builds.
