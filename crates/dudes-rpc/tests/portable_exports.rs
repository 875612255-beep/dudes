//! the re-export groups have parted company, and this is what keeps them apart.
//!
//! The re-exports from `server` are split by platform: the operator credential's **name** is unconditional, because it is a fact about the
//! layout of a runtime directory that every platform has; the three `*_permissions` functions stay
//! Unix-only, because a file mode is exactly what does not generalise. Gating the name alongside
//! the modes would make `bins/dudes/src/credential.rs` and `bins/duded/src/socket.rs` fail to
//! resolve for `x86_64-pc-windows-msvc`, on lines that had nothing to say about a mode.
//!
//! # Why this is a source scan rather than a compile-time assertion
//!
//! **On Unix there is nothing to observe.** Every one of those names resolves here whether it is
//! gated or not, so no `use dudes_rpc::OPERATOR_CREDENTIAL_NAME` — in this file or anywhere — goes
//! red when the gate comes back. The only executable detector is a cross-compile, and the only
//! venue that runs one is a Monday cron. A re-gate would therefore sit green on every
//! developer machine and in every pull request until the following week.
//!
//! So the scan reads the declarations and asserts their shape, at both of the places a gate can
//! be put back. That is a narrower claim than the cross-compile makes and it is stated as such: it
//! proves the name is re-exported and declared outside every gate this file knows how to read, not
//! that the crate compiles for Windows.
//!
//! # Two sites, two readings
//!
//! `lib.rs` is the re-export, and its re-export block is a flat list of `pub use` statements, so a
//! scan that splits on `;` sorts each one into a gated or an ungated half.
//!
//! `server.rs` is the definition, and the same scan cannot read it: `mod unix { … }` is one item
//! holding many `;`, so a run opened at the module's `#[cfg(unix)]` would close at the first `use`
//! inside it and call everything after that ungated. Moving the constant back inside that module
//! and re-exporting it through the module's own gated `pub use unix::{…}` block leaves `lib.rs`
//! untouched, so a scan of `lib.rs` alone stays green. What a parser-less
//! scan *can* read there is indentation: rustfmt indents a module body, so a top-level declaration
//! starts at column 0 and one inside `mod unix` does not; and the attributes on a declaration are
//! the code lines directly above it, which is where a gate on the constant itself would sit.
//!
//! # Two gate spellings, and no other
//!
//! `#[cfg(unix)]` and `#[cfg(target_family = "unix")]` are read. `#[cfg(not(windows))]`,
//! `#[cfg(any(unix, …))]` and every other way of saying the same thing are not, and the claim is
//! the size of that list: this file catches the regression it was written for and its one synonym;
//! the cross-compile catches the rest, a week later.

use std::fs;
use std::path::PathBuf;

/// The name that must be exported unconditionally.
const UNCONDITIONAL: &str = "OPERATOR_CREDENTIAL_NAME";

/// The two local-transport types every supported platform exports.
const LOCAL_TYPES: &[&str] = &["LocalClient", "LocalTransport"];

/// How `server.rs` declares it. The `pub const` is part of the needle so that a `use` of the name
/// inside the Unix module — there is one — does not read as the declaration.
const DECLARATION: &str = "pub const OPERATOR_CREDENTIAL_NAME";

/// The names that must stay behind the gate. A mode is a Unix concept and exporting one everywhere
/// would be a promise this crate cannot keep on the other platform.
const UNIX_ONLY: &[&str] = &[
    "operator_credential_permissions",
    "runtime_dir_permissions",
    "socket_permissions",
];

/// The attributes that open a gated item: the spelling `lib.rs` uses and its one synonym. The
/// module docs say what is deliberately not on this list.
const GATES: &[&str] = &["#[cfg(unix)]", "#[cfg(target_family = \"unix\")]"];

/// The marker that ends the library half of a source file, so the scan does not read a test.
const TEST_MODULE: &str = "\n#[cfg(test)]";

fn src(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(file)
}

/// Whether `text` carries one of the gate spellings this file reads.
fn is_gate(text: &str) -> bool {
    GATES.iter().any(|gate| text.contains(gate))
}

/// `source` with its comment lines removed, so the scan reads declarations and not prose.
///
/// **Not an optimisation — the scan is unusable without it, and this file proved that on itself.**
/// The comment that explains the split in `lib.rs` necessarily names both
/// `OPERATOR_CREDENTIAL_NAME` and `#[cfg(unix)]`, sits directly above the gated block, and put the
/// constant's name into the gated half on the first run. A guard that a maintainer cannot explain
/// their code next to is a guard that gets deleted; the same reasoning is written out at length in
/// `crates/dudes-host/tests/common/mod.rs`'s `code_lines`, which is the house pattern this follows.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `lib.rs`'s re-export section, split into `(gated, ungated)` text.
///
/// A run starts at each gate and ends at the `;` closing the item it applies to. Crude on purpose:
/// `lib.rs`'s re-export block is a flat list of `pub use` statements, and a parser here would be a
/// second thing to get wrong. [`the_scan_can_tell_a_gated_export_from_an_ungated_one`] is what
/// says the crudeness is sufficient there — and
/// [`the_declaration_scan_can_tell_where_a_constant_lives`] is what says it is insufficient for
/// `server.rs`, which is why that file gets [`declaration_in`] instead.
fn gated_and_ungated(source: &str) -> (String, String) {
    let library = source.split(TEST_MODULE).next().unwrap_or(source);
    let code = code_only(library);
    let mut gated = String::new();
    let mut ungated = String::new();
    let mut in_gate = false;
    for statement in code.split_inclusive(';') {
        if is_gate(statement) {
            in_gate = true;
        }
        if in_gate {
            gated.push_str(statement);
        } else {
            ungated.push_str(statement);
        }
        // A `;` closes whatever item was open, gate included.
        if statement.contains(';') {
            in_gate = false;
        }
    }
    (gated, ungated)
}

/// Where a source file declares the constant.
#[derive(Debug, PartialEq, Eq)]
enum Declared {
    /// A top-level item with no gate on it: the required shape.
    Unconditionally,
    /// Indented, so inside some item — `mod unix { … }` being the one that matters.
    InsideAnItem,
    /// At the top level, with a gate attribute directly on it.
    UnderAGate,
}

/// How `source` declares the constant, or `None` when it does not.
///
/// `None` is its own answer rather than a failure of one of the other two: a renamed constant is a
/// different defect from a gated one, and the assertion that reads this says which it found.
fn declaration_in(source: &str) -> Option<Declared> {
    let code = code_only(source);
    let lines: Vec<&str> = code.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.trim_start().starts_with(DECLARATION))?;
    if !lines[at].starts_with(DECLARATION) {
        return Some(Declared::InsideAnItem);
    }
    // The item's attributes are the code lines directly above it; `code_only` has already removed
    // the doc comments that would otherwise sit between them and the item.
    let gated = lines[..at]
        .iter()
        .rev()
        .take_while(|line| line.starts_with("#["))
        .any(|line| is_gate(line));
    Some(if gated {
        Declared::UnderAGate
    } else {
        Declared::Unconditionally
    })
}

/// The credential's name is exported on every platform.
#[test]
fn the_operator_credential_name_is_exported_unconditionally() {
    let source = fs::read_to_string(src("lib.rs")).expect("read lib.rs");
    let (gated, ungated) = gated_and_ungated(&source);
    assert!(
        ungated.contains(UNCONDITIONAL),
        "`{UNCONDITIONAL}` must be re-exported outside every gated block in lib.rs; the CLI reads it to find the credential it presents, on both platforms"
    );
    assert!(
        !gated.contains(UNCONDITIONAL),
        "`{UNCONDITIONAL}` appears inside a gated block in lib.rs; that is the defect this test \
         exists for, and it is invisible to every check that runs on a Unix host"
    );
}

/// Consumers get one client and one listener name without selecting a platform.
#[test]
fn the_local_transport_types_are_exported_unconditionally() {
    let source = fs::read_to_string(src("lib.rs")).expect("read lib.rs");
    let (gated, ungated) = gated_and_ungated(&source);
    for name in LOCAL_TYPES {
        assert!(
            ungated.contains(name),
            "`{name}` must be re-exported outside the transport cfg so Windows and Unix consumers use one name"
        );
        assert!(
            !gated.contains(name),
            "`{name}` is still inside a Unix-only re-export block"
        );
    }
}

/// The daemon and CLI depend on the local abstractions, never either concrete stream.
#[test]
fn transport_consumers_name_no_platform_stream() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|path| path.parent())
        .expect("dudes-rpc is under crates/ in the workspace");
    let consumers = [
        workspace.join("bins/dudes/src/run.rs"),
        workspace.join("bins/duded/src/boot.rs"),
        workspace.join("bins/duded/src/runtime.rs"),
    ];
    let concrete = ["UnixStream", "PipeStream", "PipeListenerOptions"];
    for path in consumers {
        let source = fs::read_to_string(&path).expect("read transport consumer");
        let code = code_only(&source);
        for name in concrete {
            assert!(
                !code.contains(name),
                "{} names concrete transport `{name}`; use LocalClient or LocalTransport",
                path.display()
            );
        }
    }
}

/// Remote rejection is an explicit construction-site value, not a dependency default.
#[test]
fn the_windows_listener_explicitly_rejects_remote_clients() {
    let source = fs::read_to_string(src("server.rs")).expect("read server.rs");
    let code = code_only(&source);
    assert!(
        code.contains(".accept_remote(false)"),
        "the named-pipe construction site must spell `accept_remote: false` explicitly"
    );

    let inherited_default = code.replace(".accept_remote(false)", "");
    assert!(
        !inherited_default.contains(".accept_remote(false)"),
        "control: removing the explicit call must be visible to this source check"
    );
}

/// The credential's name is *declared* on every platform, which the test above cannot see.
///
/// `lib.rs` re-exports `server::OPERATOR_CREDENTIAL_NAME`, and that path resolves on a Unix host
/// whether `server.rs` declares the constant at its top level or inside `mod unix` and re-exports
/// it through the module's own `#[cfg(unix)] pub use unix::{…}` block. The first shape is required;
/// the second is the regression, and it leaves `lib.rs` — and every assertion that reads
/// `lib.rs` — exactly as it was.
#[test]
fn the_operator_credential_name_is_declared_outside_the_unix_module() {
    let source = fs::read_to_string(src("server.rs")).expect("read server.rs");
    assert_eq!(
        declaration_in(&source),
        Some(Declared::Unconditionally),
        "`{DECLARATION}` must be a top-level, ungated item in server.rs. \
         Declared inside `mod unix` and re-exported through that module's gated `pub use` block \
         it resolves on this host, vanishes on Windows, and leaves lib.rs untouched"
    );
}

/// The three modes stay Unix-only.
///
/// The other half of the rule, and the half that makes the tests above mean something: a scan
/// that simply required every name to be ungated would "pass" a `lib.rs` with no gates at all,
/// which is a different bug and a worse one — it would export `socket_permissions` as part of the
/// Windows API and force a body for it.
#[test]
fn the_permission_accessors_stay_behind_the_unix_gate() {
    let source = fs::read_to_string(src("lib.rs")).expect("read lib.rs");
    let (gated, ungated) = gated_and_ungated(&source);
    for name in UNIX_ONLY {
        assert!(
            gated.contains(name),
            "`{name}` must stay behind `#[cfg(unix)]`: a file mode is a Unix concept and this \
             crate cannot answer for one on Windows"
        );
        assert!(
            !ungated.contains(name),
            "`{name}` is exported outside `#[cfg(unix)]`, which promises the Windows build a \
             function that has no meaning there"
        );
    }
}

/// The control: the split above is a fact about `lib.rs`, not about a scan that sorts nothing.
///
/// Both directions are planted, because a one-directional control passes for a scan that puts
/// everything in one bucket. A scan that called everything gated would satisfy "the modes are
/// gated"; a scan that called everything ungated would satisfy "the name is ungated". Only a
/// sample containing both, sorted correctly, rules out both.
#[test]
fn the_scan_can_tell_a_gated_export_from_an_ungated_one() {
    let sample = "pub use server::{Listener, OPERATOR_CREDENTIAL_NAME};\n\
                  #[cfg(unix)]\n\
                  pub use server::{socket_permissions};\n\
                  pub use tail::Subscribers;\n";
    let (gated, ungated) = gated_and_ungated(sample);
    assert!(
        ungated.contains(UNCONDITIONAL) && !gated.contains(UNCONDITIONAL),
        "control: an ungated export must land in the ungated half, got gated={gated:?}"
    );
    assert!(
        gated.contains("socket_permissions") && !ungated.contains("socket_permissions"),
        "control: a gated export must land in the gated half, got ungated={ungated:?}"
    );
    assert!(
        ungated.contains("Subscribers"),
        "control: the gate closes at its own statement and does not swallow the next one"
    );

    // The prose control, and it is not hypothetical: `lib.rs`'s own explanatory comment names both
    // needles and sits directly above the gated block, which put the constant in the wrong half
    // until `code_only` was added. A scan that reads comments reports on the explanation rather
    // than on the code.
    let commented = format!(
        "// {UNCONDITIONAL} moved out of the #[cfg(unix)] block below.\n#[cfg(unix)]\npub use \
         server::{{socket_permissions}};\n"
    );
    let (gated, ungated) = gated_and_ungated(&commented);
    assert!(
        !gated.contains(UNCONDITIONAL) && !ungated.contains(UNCONDITIONAL),
        "control: a comment naming the constant must not be scanned at all, in either half"
    );

    // The other direction, and the one that catches a scan wired backwards: moving the name under
    // the gate must move it between the halves — in either spelling of the gate.
    for gate in GATES {
        let regressed =
            format!("{gate}\npub use server::{{OPERATOR_CREDENTIAL_NAME, socket_permissions}};\n");
        let (gated, ungated) = gated_and_ungated(&regressed);
        assert!(
            gated.contains(UNCONDITIONAL) && !ungated.contains(UNCONDITIONAL),
            "control: the exact regression this file exists to catch, spelled `{gate}`, must sort \
             as gated"
        );
    }
}

/// The control for the `server.rs` reading, planted in every shape it has to tell apart.
///
/// The first sample is the reason that reading exists, and it is asserted from both sides: the
/// statement scan calls the constant in it *ungated*, because the module's first `use` closed the
/// gate long before the declaration — so a file that only ran the statement scan over `server.rs`
/// would be a second green test for the same regression.
#[test]
fn the_declaration_scan_can_tell_where_a_constant_lives() {
    let declaration = format!("{DECLARATION}: &str = \"operator.cred\";");

    let inside = [
        "#[cfg(unix)]",
        "mod unix {",
        "    use std::fs;",
        "    use std::io;",
        "",
        &format!("    {declaration}"),
        "}",
        "",
        "#[cfg(unix)]",
        "pub use unix::{OPERATOR_CREDENTIAL_NAME, socket_permissions};",
    ]
    .join("\n");
    let (_, ungated) = gated_and_ungated(&inside);
    assert!(
        ungated.contains(DECLARATION),
        "control: the statement scan is blind to a declaration inside `mod unix`, which is why \
         there is a second reading; got ungated={ungated:?}"
    );
    assert_eq!(
        declaration_in(&inside),
        Some(Declared::InsideAnItem),
        "control: an indented declaration is inside an item"
    );

    let top = [
        declaration.as_str(),
        "",
        "#[cfg(unix)]",
        "mod unix {",
        "    use super::OPERATOR_CREDENTIAL_NAME;",
        "}",
    ]
    .join("\n");
    assert_eq!(
        declaration_in(&top),
        Some(Declared::Unconditionally),
        "control: a top-level declaration with no attribute is unconditional, and the `use` of \
         the name inside the module is not mistaken for it"
    );

    for gate in GATES {
        let under = [*gate, declaration.as_str()].join("\n");
        assert_eq!(
            declaration_in(&under),
            Some(Declared::UnderAGate),
            "control: `{gate}` directly on the declaration is a gate"
        );
        // Docs between the attribute and the item: the shape a gate lands in when it is written
        // above an already-documented constant.
        let documented = [*gate, "/// The name.", declaration.as_str()].join("\n");
        assert_eq!(
            declaration_in(&documented),
            Some(Declared::UnderAGate),
            "control: `{gate}` above the doc comment is still a gate on the item"
        );
    }

    let harmless = ["#[doc(hidden)]", declaration.as_str()].join("\n");
    assert_eq!(
        declaration_in(&harmless),
        Some(Declared::Unconditionally),
        "control: an attribute that is not a gate is not read as one"
    );

    let commented = format!("// {declaration} used to live here.\n");
    assert_eq!(
        declaration_in(&commented),
        None,
        "control: a comment naming the declaration is not a declaration"
    );
}
