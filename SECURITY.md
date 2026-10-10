# Security policy

Dudes spawns coding-agent CLIs, listens on a local socket, and mints per-agent tokens for hook
callbacks. Bugs on those edges are security bugs. Please report them privately.

## Supported versions

Dudes is pre-1.0 and has no releases yet. Security fixes land on the `main` branch. To get them,
build from the latest `main`.

## Reporting a vulnerability

Report vulnerabilities privately to [alex.nthnz@gmail.com](mailto:alex.nthnz@gmail.com).
Alex Nguyen monitors this address.

GitHub private vulnerability reporting requires a public repository with the feature enabled.
Alternatively, if the repository's **Advisories** page offers **Report a vulnerability**, use it to
create a private report.
Do not open a public issue or pull request for a security problem.

Please include: the affected version or commit, the platform, a reproduction (a journal excerpt,
a config, or a command sequence), and the impact as you understand it.

What to expect:

- acknowledgement within 3 business days;
- an initial assessment within 10 business days;
- a fix or mitigation, a CVE if warranted, and credit in the fix's advisory or commit message
  (unless you prefer not to be named). We coordinate the disclosure date with you; we aim for 90
  days at most.

## In scope

- the daemon's local RPC socket and its peer-credential authentication;
- hook tokens: minting, transport, validation;
- command construction and process spawning (argument handling, environment, working directory);
- child process ownership (process groups / job objects) and cleanup;
- the journal and its projections (integrity, replay, file permissions);
- the mailbox (attribution of `from`, delivery, quarantine);
- installers and distribution artefacts.

## Out of scope

- vulnerabilities in the coding-agent CLIs Dudes supervises (report those to their vendors);
- attacks that require the attacker to already run code as the same user on the same machine,
  unless they cross a boundary Dudes claims to enforce;
- an agent or a completion check using the access that the README's
  [What agents can do on your machine](README.md#what-agents-can-do-on-your-machine) describes;
- denial of service by exhausting a budget you configured.

## Supply chain

`cargo xtask dist` builds binaries with `cargo build --release --locked`. On macOS and Linux,
it writes an archive and records its SHA-256 in `SHA256SUMS`. The install script checks the
downloaded archive against that file. This detects a mismatch with the listed hash; it does not
authenticate the publisher. The distribution command does not generate release signatures, an SBOM,
or build provenance attestations.

CI is configured to run `cargo deny --locked check` for RustSec advisories, licences, bans and
sources on pull requests and weekly. Dependabot is configured to open update pull requests for
Cargo and GitHub Actions dependencies. These configurations do not prove that checks ran or passed
for a particular commit.
