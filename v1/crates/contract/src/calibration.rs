//! Private calibration replies carry credentials; public snapshots/events do not.
use prysm_core::ColorCorrection;
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileId(pub NonZeroU64);

/// Opaque credentials. Deliberately no Debug/Display; generation belongs to the runtime.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingToken(pub [u8; 32]);

#[derive(Clone, PartialEq, Eq)]
pub struct CalibrationToken(pub [u8; 32]);

#[derive(Debug, Clone)]
pub struct CalibrationProfile {
    pub id: ProfileId,
    pub name: String,
    pub correction: ColorCorrection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationStep {
    Choose,
    Response,
    WhiteBalance,
    Intensity,
    Colors,
    Mixed,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationStatus {
    Idle,
    Pairing,
    Active,
}

#[derive(Clone)]
pub enum CalibrationCommand {
    Begin,
    Pair {
        pairing: PairingToken,
    },
    Poll {
        token: CalibrationToken,
        compare: bool,
        playing: bool,
    },
    Select {
        token: CalibrationToken,
        profile: Option<ProfileId>,
    },
    Adjust {
        token: CalibrationToken,
        sequence: u64,
        correction: ColorCorrection,
        step: CalibrationStep,
    },
    Save {
        token: CalibrationToken,
        name: String,
    },
    End {
        token: CalibrationToken,
    },
}

#[derive(Debug, Clone)]
pub struct CalibrationView {
    pub status: CalibrationStatus,
    pub step: CalibrationStep,
    pub correction: ColorCorrection,
    pub profile: Option<ProfileId>,
}

/// Returned only to the requesting client, never through the public event stream.
#[derive(Clone)]
pub enum CalibrationReply {
    Began {
        pairing: PairingToken,
        view: CalibrationView,
    },
    Paired {
        token: CalibrationToken,
        view: CalibrationView,
    },
    Updated(CalibrationView),
    Saved(CalibrationProfile),
    Ended,
}
