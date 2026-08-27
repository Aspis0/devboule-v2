//! Open-set capabilities. Unknown names must not fail the handshake.

use serde::{Deserialize, Serialize};

/// A capability name. Serialized as a plain JSON string.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct Capability(String);

impl Capability {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Capability {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

/// Intersection, preserving daemon order (the server's advertised set).
pub fn intersect_capabilities(client: &[Capability], daemon: &[Capability]) -> Vec<Capability> {
    daemon
        .iter()
        .filter(|capability| client.contains(capability))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_capability_deserializes_as_string() {
        let capability: Capability = serde_json::from_str("\"typed_permissions\"").expect("json");
        assert_eq!(capability.as_str(), "typed_permissions");
    }

    #[test]
    fn intersection_keeps_daemon_order_and_drops_unknown() {
        let client = vec![Capability::new("ping"), Capability::new("sessions")];
        let daemon = vec![
            Capability::new("ping"),
            Capability::new("status"),
            Capability::new("shutdown"),
        ];
        let agreed = intersect_capabilities(&client, &daemon);
        assert_eq!(agreed, vec![Capability::new("ping")]);
    }
}
