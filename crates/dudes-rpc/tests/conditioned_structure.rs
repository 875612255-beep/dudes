//! Structural checks for the non-forgeable applied-effect capability.

use std::fs;
use std::path::{Path, PathBuf};

const CONSTRUCTOR: &str = "AppliedEffect {";

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files
}

fn library_text(text: &str) -> &str {
    text.split("\n#[cfg(test)]").next().unwrap_or(text)
}

fn constructors(text: &str) -> usize {
    text.matches(CONSTRUCTOR).count()
}

#[test]
fn only_the_router_constructs_applied_effect() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for path in source_files(&root) {
        let text = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        let count = constructors(library_text(&text));
        if count != 0 {
            found.push((path, count));
        }
    }
    assert_eq!(
        found.len(),
        1,
        "one production module constructs the capability"
    );
    assert!(
        found[0].0.ends_with("conditioned.rs"),
        "the constructor belongs to the central router: {found:?}"
    );
    assert_eq!(found[0].1, 1, "the router consumes Apply in one place");

    let planted = "fn forge() { let _ = AppliedEffect { params, target, provenance, actor }; }";
    assert_eq!(
        constructors(planted),
        1,
        "positive control: the same scan detects an ordinary out-of-router constructor"
    );
}

#[test]
fn applied_effect_has_private_fields_and_no_clone_or_constructor() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/conditioned.rs");
    let text = fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let (before_definition, after_definition) = text
        .split_once("pub struct AppliedEffect<M> {")
        .expect("the applied-effect definition");
    let definition = after_definition
        .split_once("}\n")
        .map(|(body, _)| body)
        .expect("the applied-effect definition");
    assert!(
        !definition
            .lines()
            .any(|line| line.trim_start().starts_with("pub ")),
        "a field exposed outside the router makes the capability forgeable"
    );
    let implementation = text
        .split("impl<M> AppliedEffect<M> {")
        .nth(1)
        .and_then(|tail| tail.split_once("/// The identity-bearing view"))
        .map(|(body, _)| body)
        .expect("the applied-effect implementation");
    assert!(!implementation.contains("pub fn new"));
    assert!(
        !implementation.contains("pub const fn provenance(&self)")
            && !implementation.contains("pub fn provenance(&self)"),
        "an effect sink must not receive raw peer provenance"
    );
    assert!(
        !implementation.contains("pub const fn actor(&self)")
            && !implementation.contains("pub fn actor(&self)"),
        "an effect sink must not receive the router-owned actor"
    );
    assert!(!text.contains("impl<M> Clone for AppliedEffect"));
    assert!(
        !before_definition
            .rsplit("\n\n")
            .next()
            .unwrap_or_default()
            .contains("derive(Clone"),
        "AppliedEffect must not derive Clone"
    );

    let planted = "pub struct AppliedEffect<M> { pub params: M }";
    assert!(
        planted.contains("pub params"),
        "positive control: a public field is distinguishable"
    );
}

#[test]
fn only_the_persistence_operation_converts_the_router_actor() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/conditioned.rs");
    let text = fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let library = library_text(&text);

    assert!(
        library.contains("pub(crate) struct ConditionedConnectionContext"),
        "the conditioned path needs its own opaque connection context"
    );
    assert!(
        library.contains("pub struct AppliedPersistence"),
        "actor conversion belongs to a distinct persistence operation"
    );
    assert_eq!(
        library.matches("record_actor()").count(),
        1,
        "one production call site converts the opaque actor"
    );
    let persistence = library
        .split_once("impl<M> AppliedPersistence<'_, M> {")
        .map(|(_, tail)| tail)
        .expect("the applied persistence interface");
    assert!(
        persistence.contains("record_actor()"),
        "the sole conversion is inside the persistence interface"
    );

    let planted = "fn sink(actor: RouterActor) { let _ = actor.record_actor(); }";
    assert_eq!(
        planted.matches("record_actor()").count(),
        1,
        "positive control: the scan detects a sink-side conversion"
    );
}

#[test]
fn persistence_receipts_are_sealed_to_the_four_exact_acknowledgements() {
    let conditioned_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/conditioned.rs");
    let conditioned = fs::read_to_string(conditioned_path)
        .unwrap()
        .replace("\r\n", "\n");
    let conditioned = library_text(&conditioned);
    let dispatch_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dispatch.rs");
    let dispatch = fs::read_to_string(dispatch_path)
        .unwrap()
        .replace("\r\n", "\n");
    let dispatch = library_text(&dispatch);

    assert!(
        conditioned.contains("pub trait PersistenceReceipt: receipt_private::Sealed"),
        "the public bound is sealed against downstream receipt types"
    );
    assert_eq!(
        conditioned
            .matches("impl receipt_private::Sealed for")
            .count(),
        4,
        "only no-value, journal-position, exact-PTY and host-byte-outcome acknowledgements are \
         admitted"
    );
    assert!(conditioned.contains("impl receipt_private::Sealed for ()"));
    assert!(conditioned.contains("impl receipt_private::Sealed for Lsn"));
    assert!(conditioned.contains("impl receipt_private::Sealed for PtyId"));
    // The fourth carries a byte count and the host's own error text, and no identity at all —
    // which is what keeps the seal meaningful as the set grows.
    assert!(conditioned.contains("impl receipt_private::Sealed for dudes_proto::PtyWriteOutcome"));
    assert!(conditioned.contains("R: PersistenceReceipt + 'static"));
    assert!(dispatch.contains("R: PersistenceReceipt + 'static"));
    for forbidden in ["Actor", "PeerProvenance", "RouterActor", "PeerIdentity"] {
        assert!(
            !conditioned.contains(&format!("impl receipt_private::Sealed for {forbidden}")),
            "{forbidden} cannot cross persistence as a sink receipt"
        );
    }

    let planted = "impl receipt_private::Sealed for PeerProvenance {}";
    assert!(
        planted.contains("impl receipt_private::Sealed for PeerProvenance"),
        "positive control: the structural scan recognizes a forbidden receipt"
    );
}

#[test]
fn server_carries_transport_identity_into_the_conditioned_context() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server.rs");
    let text = fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let library = library_text(&text);
    assert!(
        library.contains("ConditionedConnectionContext::from_accepted_peer("),
        "the connection-bound context must be built from the accepted peer, on that connection's \
         own thread and before any request on it is read"
    );
    let accepted_inputs: String = library
        .split_once("ConditionedConnectionContext::from_accepted_peer(")
        .and_then(|(_, tail)| tail.split_once(");"))
        .map(|(arguments, _)| arguments.chars().filter(|ch| !ch.is_whitespace()).collect())
        .expect("the accepted-context inputs");
    assert_eq!(
        accepted_inputs, "peer,provenance,authority",
        "the accepted context keeps transport identity, independently resolved provenance, and \
         the daemon-owned source as separate inputs"
    );
    assert!(
        library.contains("serve_connection_on_context(conn, context, dispatcher)"),
        "the accepted context, not a reconstructible fact tuple, crosses into the thread"
    );
    let connection = library
        .split_once("fn serve_connection_on_context<C: Connection>(")
        .and_then(|(_, tail)| tail.split_once("/// Serve one already-split stream."))
        .map(|(body, _)| body)
        .expect("the connection-serving implementation");
    let revoke = connection
        .find("context.revoke();")
        .expect("explicit revocation");
    let close = connection
        .find("outbound.close();")
        .expect("outbound teardown");
    assert!(
        revoke < close,
        "authority is revoked before outbound teardown"
    );

    let planted = "serve_connection_on_context(conn, provenance, dispatcher)";
    assert!(
        planted.contains("conn, provenance"),
        "positive control: a provenance-only handoff is distinguishable"
    );
}

#[test]
fn only_the_source_constructs_operator_effect_authority() {
    const CONSTRUCTOR: &str = "Some(OperatorEffectAuthority {";

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for path in source_files(&root) {
        let text = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        let count = library_text(&text).matches(CONSTRUCTOR).count();
        if count != 0 {
            found.push((path, count));
        }
    }
    assert_eq!(found.len(), 1, "one production module constructs authority");
    assert!(found[0].0.ends_with("conditioned.rs"));
    assert_eq!(found[0].1, 1, "the source has one construction site");

    let conditioned = fs::read_to_string(root.join("conditioned.rs"))
        .unwrap()
        .replace("\r\n", "\n");
    let library = library_text(&conditioned);
    let source_implementation = library
        .split_once("impl OperatorAuthoritySource {")
        .and_then(|(_, tail)| {
            tail.split_once("/// One opaque, connection-bound, single-owner lease.")
        })
        .map(|(body, _)| body)
        .expect("the authority source implementation");
    assert!(
        source_implementation.contains(CONSTRUCTOR)
            && source_implementation
                .split_once("fn acquire(&self, provenance: PeerProvenance)")
                .is_some_and(|(_, acquire)| acquire.contains(CONSTRUCTOR)),
        "the sole construction is inside OperatorAuthoritySource::acquire"
    );

    let planted = "fn forge() { Some(OperatorEffectAuthority { live, source }) }";
    assert_eq!(
        planted.matches(CONSTRUCTOR).count(),
        1,
        "positive control: the scan detects a forged construction"
    );
}

#[test]
fn operator_effect_authority_has_no_public_clone_serialize_or_carrier() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let conditioned = fs::read_to_string(root.join("conditioned.rs"))
        .unwrap()
        .replace("\r\n", "\n");
    let library = library_text(&conditioned);
    let (before, after) = library
        .split_once("struct OperatorEffectAuthority {")
        .expect("the private authority definition");
    let definition = after
        .split_once("}\n")
        .map(|(body, _)| body)
        .expect("the authority fields");
    assert!(
        !definition
            .lines()
            .any(|line| line.trim_start().starts_with("pub ")),
        "authority fields stay private"
    );
    assert!(!before.ends_with("pub "), "the authority type is private");
    let derive = before.lines().next_back().unwrap_or_default();
    assert!(!derive.contains("Clone"));
    assert!(!derive.contains("Serialize"));
    assert!(!library.contains("impl Clone for OperatorEffectAuthority"));
    assert!(!library.contains("impl Serialize for OperatorEffectAuthority"));
    let implementation = after
        .split_once("impl OperatorEffectAuthority {")
        .and_then(|(_, tail)| tail.split_once("impl Drop for OperatorEffectAuthority"))
        .map(|(body, _)| body)
        .expect("the authority implementation");
    assert!(
        !implementation.contains("pub fn new") && !implementation.contains("pub const fn new"),
        "the authority has no public constructor"
    );

    let carriers: Vec<PathBuf> = source_files(&root)
        .into_iter()
        .filter(|path| !path.ends_with("conditioned.rs"))
        .filter(|path| {
            let text = fs::read_to_string(path).unwrap().replace("\r\n", "\n");
            library_text(&text).contains("OperatorEffectAuthority")
        })
        .collect();
    assert!(
        carriers.is_empty(),
        "the lease cannot enter Request, credentials, identity, provenance, role, or another \
         carrier: {carriers:?}"
    );

    let planted = "pub struct Request { authority: OperatorEffectAuthority }";
    assert!(
        planted.contains("OperatorEffectAuthority"),
        "positive control: a request carrier is distinguishable"
    );
}

/// `PeerResolution` carries no serialization derive, and a planted one is detected.
///
/// It is decided and consumed inside one connection frame and never crosses a wire, so a
/// `Serialize`, `Deserialize` or `JsonSchema` on it would be the first step toward a fifth classification
/// column travelling as data — which is the thing the type exists to make unspellable. The scan is
/// textual because the property is textual: a derive is either written above the item or it is
/// not.
#[test]
fn peer_resolution_carries_no_serialization_derive() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/auth.rs");
    let text = fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let library = library_text(&text);

    let item = library
        .split_once("pub enum PeerResolution {")
        .expect("the enum is declared in this file")
        .0;
    let derives = item
        .rsplit_once("#[derive(")
        .expect("the enum carries a derive list")
        .1;
    let derives = derives.split_once(")]").expect("a closed derive list").0;

    for forbidden in ["Serialize", "Deserialize", "JsonSchema"] {
        assert!(
            !derives.contains(forbidden),
            "PeerResolution derives {forbidden}; it is never serialized, and a wire type is the \
             one thing it must not become. Derives found: {derives}"
        );
    }

    let planted = "Debug, Clone, Copy, PartialEq, Eq, Serialize";
    assert!(
        planted.contains("Serialize"),
        "positive control: the scan recognises a planted serialization derive"
    );
}
