use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShipmentState {
    InTransit,
    ExceptionOpen,
    Delivered,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofOfDelivery {
    pub object_key: String,
    pub sha256: String,
    pub signed_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShipmentEvent {
    Scan { location: String },
    ExceptionRaised { reason: String },
    ExceptionResolved { resolution: String },
    ProofAttached { proof: ProofOfDelivery },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shipment {
    pub tracking_id: String,
    pub state: ShipmentState,
    pub events: Vec<ShipmentEvent>,
    pub proof: Option<ProofOfDelivery>,
    exception_open: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ShipmentError {
    #[error("an open exception blocks delivery")]
    OpenException,
    #[error("proof of delivery is required")]
    MissingProof,
}

impl Shipment {
    pub fn new(tracking_id: impl Into<String>) -> Self {
        Self {
            tracking_id: tracking_id.into(),
            state: ShipmentState::InTransit,
            events: Vec::new(),
            proof: None,
            exception_open: false,
        }
    }

    pub fn record(&mut self, event: ShipmentEvent) {
        match &event {
            ShipmentEvent::ExceptionRaised { .. } => {
                self.exception_open = true;
                self.state = ShipmentState::ExceptionOpen;
            }
            ShipmentEvent::ExceptionResolved { .. } => {
                self.exception_open = false;
                self.state = ShipmentState::InTransit;
            }
            ShipmentEvent::ProofAttached { proof } => self.proof = Some(proof.clone()),
            ShipmentEvent::Scan { .. } => {}
        }
        self.events.push(event);
    }

    pub fn complete_delivery(&mut self) -> Result<(), ShipmentError> {
        if self.exception_open {
            return Err(ShipmentError::OpenException);
        }
        if self.proof.is_none() {
            return Err(ShipmentError::MissingProof);
        }
        self.state = ShipmentState::Delivered;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_exception_and_proof_allow_delivery() {
        let mut shipment = Shipment::new("SHP-2048");
        shipment.record(ShipmentEvent::ExceptionRaised {
            reason: "dock closed".into(),
        });
        assert_eq!(
            shipment.complete_delivery(),
            Err(ShipmentError::OpenException)
        );

        shipment.record(ShipmentEvent::ExceptionResolved {
            resolution: "receiver reopened dock".into(),
        });
        assert_eq!(
            shipment.complete_delivery(),
            Err(ShipmentError::MissingProof)
        );

        shipment.record(ShipmentEvent::ProofAttached {
            proof: ProofOfDelivery {
                object_key: "pod/SHP-2048.jpg".into(),
                sha256: "cafe2048".into(),
                signed_by: "M. Rivera".into(),
            },
        });
        shipment.complete_delivery().unwrap();
        assert_eq!(shipment.state, ShipmentState::Delivered);
    }
}
