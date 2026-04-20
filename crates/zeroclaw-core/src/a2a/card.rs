//! Static `AgentCard` builders for Sam and Walter.
//!
//! Served at `/.well-known/agent-card.json` by each binary.

use ra2a::types::{AgentCapabilities, AgentCard, AgentInterface, TransportProtocol};

pub fn sam_agent_card(base_url: &str) -> AgentCard {
    let mut card = AgentCard::new(
        "Sam",
        "ZeroClaw: personal assistant, delegator, Signal + Vikunja coordinator",
        vec![AgentInterface::new(
            base_url,
            TransportProtocol::from(TransportProtocol::JSONRPC),
        )],
    );
    card.capabilities = AgentCapabilities {
        streaming: Some(false),
        push_notifications: Some(true),
        ..Default::default()
    };
    card
}

pub fn walter_agent_card(base_url: &str) -> AgentCard {
    let mut card = AgentCard::new(
        "Walter",
        "ZeroClaw: read-only Kubernetes cluster observer",
        vec![AgentInterface::new(
            base_url,
            TransportProtocol::from(TransportProtocol::JSONRPC),
        )],
    );
    card.capabilities = AgentCapabilities {
        streaming: Some(false),
        push_notifications: Some(true),
        ..Default::default()
    };
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sam_card_has_push_notifications() {
        let card = sam_agent_card("http://localhost:3000");
        assert_eq!(card.capabilities.push_notifications, Some(true));
        assert_eq!(card.name, "Sam");
        assert_eq!(card.supported_interfaces.len(), 1);
        assert!(card.supported_interfaces[0].protocol_binding.is_jsonrpc());
    }

    #[test]
    fn walter_card_has_push_notifications() {
        let card = walter_agent_card("http://localhost:3000");
        assert_eq!(card.capabilities.push_notifications, Some(true));
        assert_eq!(card.name, "Walter");
    }
}
