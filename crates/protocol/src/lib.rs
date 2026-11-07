//! ClawDot satellite protocol.
//!
//! The only contract a hardware port must implement (spec 0003, ADR-0005, ADR-0008).

/// Current protocol version as `(major, minor)`. Unstable until spec 0003 is Accepted.
pub const PROTOCOL_VERSION: (u16, u16) = (0, 1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_is_pre_stable() {
        // REQ-SAT-009: major 0 means the wire format may still change.
        assert_eq!(PROTOCOL_VERSION.0, 0);
    }
}
