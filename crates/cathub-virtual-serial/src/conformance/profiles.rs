use super::{ApplicationProfile, CaseId, ProfileCase, Requirement};

const BASE_CAT: &[ProfileCase] = &[
    required(CaseId::SynchronousIo, "CatHub issue #6 acceptance scope"),
    required(CaseId::OverlappedIo, "CatHub issue #6 acceptance scope"),
    required(
        CaseId::CancelPendingRead,
        "CatHub issue #6 acceptance scope",
    ),
    required(CaseId::ReadTimeout, "CatHub issue #6 acceptance scope"),
    required(CaseId::PurgeReceive, "CatHub issue #6 acceptance scope"),
    required(CaseId::WaitCommEvent, "CatHub issue #6 acceptance scope"),
    required(
        CaseId::SerialConfiguration,
        "CatHub issue #6 acceptance scope",
    ),
    optional(CaseId::ModemControl, "client trace required"),
    required(CaseId::QueueStatus, "CatHub issue #6 acceptance scope"),
];

const WINKYER: &[ProfileCase] = &[
    required(CaseId::SynchronousIo, "CatHub issue #6 acceptance scope"),
    required(CaseId::OverlappedIo, "CatHub issue #6 acceptance scope"),
    required(
        CaseId::CancelPendingRead,
        "CatHub issue #6 acceptance scope",
    ),
    required(CaseId::ReadTimeout, "CatHub issue #6 acceptance scope"),
    required(CaseId::PurgeReceive, "CatHub issue #6 acceptance scope"),
    required(CaseId::WaitCommEvent, "CatHub issue #6 acceptance scope"),
    required(
        CaseId::SerialConfiguration,
        "WinKeyer host format 1200 8-N-2",
    ),
    optional(CaseId::ModemControl, "client trace required"),
    required(CaseId::QueueStatus, "CatHub issue #6 acceptance scope"),
];

const PROFILES: &[ApplicationProfile] = &[
    ApplicationProfile {
        name: "hdsdr-omnirig",
        client: "HDSDR through OmniRig, TS-2000 dialect",
        line_format: "Client-configured CAT format",
        cases: BASE_CAT,
    },
    ApplicationProfile {
        name: "n1mm-radio",
        client: "N1MM Logger+ radio CAT, TS-590 dialect",
        line_format: "Client-configured CAT format",
        cases: BASE_CAT,
    },
    ApplicationProfile {
        name: "arcp-590",
        client: "Kenwood ARCP-590",
        line_format: "Client-configured CAT format",
        cases: BASE_CAT,
    },
    ApplicationProfile {
        name: "n1mm-winkeyer",
        client: "N1MM Logger+ WinKeyer",
        line_format: "1200 baud, 8 data bits, no parity, 2 stop bits",
        cases: WINKYER,
    },
    ApplicationProfile {
        name: "wktools",
        client: "WKTools maintenance",
        line_format: "1200 baud, 8 data bits, no parity, 2 stop bits",
        cases: WINKYER,
    },
];

const fn required(id: CaseId, evidence: &'static str) -> ProfileCase {
    ProfileCase {
        id,
        requirement: Requirement::Required,
        evidence,
    }
}

const fn optional(id: CaseId, evidence: &'static str) -> ProfileCase {
    ProfileCase {
        id,
        requirement: Requirement::Optional,
        evidence,
    }
}

/// Return all supported application profiles.
#[must_use]
pub fn profiles() -> &'static [ApplicationProfile] {
    PROFILES
}

/// Find a profile by its stable name.
#[must_use]
pub fn find_profile(name: &str) -> Option<&'static ApplicationProfile> {
    PROFILES.iter().find(|profile| profile.name == name)
}
