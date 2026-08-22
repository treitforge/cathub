//! Idempotent provisioning for CatHub-owned UMDF virtual serial endpoints.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::Config;

/// One endpoint identity compiled into the CatHub UMDF package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EndpointDefinition {
    stable_id: &'static str,
    kind: EndpointKind,
    hardware_id: &'static str,
    display_name: &'static str,
    default_port: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EndpointKind {
    Cat,
    Winkeyer,
}

const ENDPOINTS: [EndpointDefinition; 6] = [
    EndpointDefinition {
        stable_id: "cathub-default",
        kind: EndpointKind::Cat,
        hardware_id: r"ROOT\CATHUB_VIRTUAL_SERIAL",
        display_name: "CatHub Virtual Serial Port",
        default_port: "COM21",
    },
    EndpointDefinition {
        stable_id: "hdsdr-cat",
        kind: EndpointKind::Cat,
        hardware_id: r"ROOT\CATHUB_HDSDR_CAT",
        display_name: "CatHub HDSDR CAT Port",
        default_port: "COM11",
    },
    EndpointDefinition {
        stable_id: "n1mm-cat",
        kind: EndpointKind::Cat,
        hardware_id: r"ROOT\CATHUB_N1MM_CAT",
        display_name: "CatHub N1MM CAT Port",
        default_port: "COM21",
    },
    EndpointDefinition {
        stable_id: "arcp590-cat",
        kind: EndpointKind::Cat,
        hardware_id: r"ROOT\CATHUB_ARCP590_CAT",
        display_name: "CatHub ARCP-590 CAT Port",
        default_port: "COM31",
    },
    EndpointDefinition {
        stable_id: "n1mm-winkeyer",
        kind: EndpointKind::Winkeyer,
        hardware_id: r"ROOT\CATHUB_N1MM_WINKEYER",
        display_name: "CatHub N1MM WinKeyer Port",
        default_port: "COM41",
    },
    EndpointDefinition {
        stable_id: "wktools",
        kind: EndpointKind::Winkeyer,
        hardware_id: r"ROOT\CATHUB_WKTOOLS",
        display_name: "CatHub WKTools Port",
        default_port: "COM43",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DesiredEndpoint {
    stable_id: String,
    kind: EndpointKind,
    hardware_id: String,
    display_name: String,
    com_port: String,
    configured_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct InstalledEndpoint {
    stable_id: String,
    kind: EndpointKind,
    hardware_id: String,
    instance_id: String,
    display_name: String,
    com_port: Option<String>,
    authorized_for_current_user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PortClaim {
    com_port: String,
    owner: String,
    cathub_owned: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SystemSnapshot {
    owned: Vec<InstalledEndpoint>,
    claims: Vec<PortClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum ProvisionAction {
    Create {
        stable_id: String,
        hardware_id: String,
        com_port: String,
    },
    Reassign {
        stable_id: String,
        instance_id: String,
        from: Option<String>,
        to: String,
    },
    Authorize {
        stable_id: String,
        instance_id: String,
        com_port: String,
    },
    Retain {
        stable_id: String,
        instance_id: String,
        com_port: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ProvisionPlan {
    desired: Vec<DesiredEndpoint>,
    actions: Vec<ProvisionAction>,
    conflicts: Vec<PortClaim>,
}

impl ProvisionPlan {
    pub(crate) fn is_applicable(&self) -> bool {
        self.conflicts.is_empty()
    }

    pub(crate) fn requires_changes(&self) -> bool {
        self.actions
            .iter()
            .any(|action| !matches!(action, ProvisionAction::Retain { .. }))
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct StatusReport {
    platform: &'static str,
    owned_endpoints: Vec<InstalledEndpoint>,
    com_claims: Vec<PortClaim>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ApplyReport {
    changed: bool,
    reboot_required: bool,
    plan: ProvisionPlan,
    status: StatusReport,
}

#[derive(Debug, Serialize)]
pub(crate) struct RemoveReport {
    removed: Vec<InstalledEndpoint>,
    reboot_required: bool,
}

pub(crate) trait TextReport {
    fn render_text(&self) -> String;
}

impl TextReport for StatusReport {
    fn render_text(&self) -> String {
        let mut lines = vec![format!(
            "CatHub virtual serial status: {} owned endpoint(s)",
            self.owned_endpoints.len()
        )];
        for endpoint in &self.owned_endpoints {
            lines.push(format!(
                "  {}: {} ({}) [{}]",
                endpoint.stable_id,
                endpoint.com_port.as_deref().unwrap_or("unassigned"),
                endpoint.instance_id,
                endpoint.display_name
            ));
            if !endpoint.authorized_for_current_user {
                lines.push("    Private daemon access requires owner reconciliation.".to_string());
            }
        }
        if self.owned_endpoints.is_empty() {
            lines.push("  No CatHub-owned devices are installed.".to_string());
        }
        lines.push(format!(
            "COM Name Arbiter/PnP claims: {}",
            self.com_claims.len()
        ));
        for claim in &self.com_claims {
            lines.push(format!("  {}: {}", claim.com_port, claim.owner));
        }
        lines.join("\n")
    }
}

impl TextReport for ProvisionPlan {
    fn render_text(&self) -> String {
        let mut lines = vec![format!(
            "CatHub virtual serial plan: {} endpoint(s), {} conflict(s)",
            self.desired.len(),
            self.conflicts.len()
        )];
        for action in &self.actions {
            let line = match action {
                ProvisionAction::Create {
                    stable_id,
                    hardware_id,
                    com_port,
                } => format!("  CREATE {stable_id} as {com_port} ({hardware_id})"),
                ProvisionAction::Reassign {
                    stable_id,
                    instance_id,
                    from,
                    to,
                } => format!(
                    "  REASSIGN {stable_id} from {} to {to} ({instance_id})",
                    from.as_deref().unwrap_or("unassigned")
                ),
                ProvisionAction::Retain {
                    stable_id,
                    instance_id,
                    com_port,
                } => format!("  RETAIN {stable_id} as {com_port} ({instance_id})"),
                ProvisionAction::Authorize {
                    stable_id,
                    instance_id,
                    com_port,
                } => format!("  AUTHORIZE {stable_id} on {com_port} ({instance_id})"),
            };
            lines.push(line);
        }
        for conflict in &self.conflicts {
            lines.push(format!(
                "  CONFLICT {} is owned by {}",
                conflict.com_port, conflict.owner
            ));
        }
        lines.join("\n")
    }
}

impl TextReport for ApplyReport {
    fn render_text(&self) -> String {
        format!(
            "{}\nApplied: changed={}, reboot_required={}\n{}",
            self.plan.render_text(),
            self.changed,
            self.reboot_required,
            self.status.render_text()
        )
    }
}

impl TextReport for RemoveReport {
    fn render_text(&self) -> String {
        let mut lines = vec![format!(
            "Removed {} CatHub-owned endpoint(s); reboot_required={}",
            self.removed.len(),
            self.reboot_required
        )];
        for endpoint in &self.removed {
            lines.push(format!(
                "  {} ({})",
                endpoint.stable_id, endpoint.instance_id
            ));
        }
        lines.join("\n")
    }
}

pub(crate) fn status() -> Result<StatusReport, String> {
    let snapshot = platform::snapshot()?;
    Ok(StatusReport {
        platform: std::env::consts::OS,
        owned_endpoints: snapshot.owned,
        com_claims: snapshot.claims,
    })
}

pub(crate) fn plan(config: &Config) -> Result<ProvisionPlan, String> {
    let desired = desired_endpoints(config)?;
    plan_from_snapshot(desired, &platform::snapshot()?)
}

pub(crate) fn apply(config: &Config, inf_path: &Path) -> Result<ApplyReport, String> {
    let inf_path = canonical_inf_path(inf_path)?;
    let desired = desired_endpoints(config)?;
    let before = platform::snapshot()?;
    let plan = plan_from_snapshot(desired, &before)?;
    if !plan.is_applicable() {
        return Err(format_conflicts(&plan.conflicts));
    }
    let changed = plan.requires_changes();
    let reboot_required = platform::apply(&plan, &inf_path)?;
    let after = platform::snapshot()?;
    let verification = plan_from_snapshot(plan.desired.clone(), &after)?;
    if !verification.is_applicable() || verification.requires_changes() {
        return Err("provisioning completed but the resulting PnP/COM state does not match the requested plan".to_string());
    }
    Ok(ApplyReport {
        changed,
        reboot_required,
        plan,
        status: StatusReport {
            platform: std::env::consts::OS,
            owned_endpoints: after.owned,
            com_claims: after.claims,
        },
    })
}

pub(crate) fn remove(stable_ids: &[String]) -> Result<RemoveReport, String> {
    let requested = stable_ids
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for stable_id in &requested {
        definition(stable_id)
            .ok_or_else(|| format!("unknown CatHub virtual endpoint `{stable_id}`"))?;
    }
    let snapshot = platform::snapshot()?;
    let removed = snapshot
        .owned
        .into_iter()
        .filter(|endpoint| requested.is_empty() || requested.contains(&endpoint.stable_id))
        .collect::<Vec<_>>();
    let reboot_required = platform::remove(&removed)?;
    Ok(RemoveReport {
        removed,
        reboot_required,
    })
}

fn canonical_inf_path(path: &Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("resolving driver INF `{}`: {error}", path.display()))?;
    if !path.is_file()
        || path
            .extension()
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("inf"))
    {
        return Err(format!(
            "driver path must name an existing .inf file: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn desired_endpoints(config: &Config) -> Result<Vec<DesiredEndpoint>, String> {
    let mut desired = Vec::new();
    for endpoint in &config.serial_endpoint {
        if let Some(stable_id) = endpoint.virtual_endpoint.as_deref() {
            desired.push(desired_endpoint(
                stable_id,
                EndpointKind::Cat,
                &endpoint.name,
                endpoint.application_transport.as_deref(),
            )?);
        }
    }
    for endpoint in &config.winkeyer_endpoint {
        if let Some(stable_id) = endpoint.virtual_endpoint.as_deref() {
            desired.push(desired_endpoint(
                stable_id,
                EndpointKind::Winkeyer,
                &endpoint.name,
                endpoint.application_transport.as_deref(),
            )?);
        }
    }
    let mut stable_ids = BTreeSet::new();
    let mut ports = BTreeSet::new();
    for endpoint in &desired {
        if !stable_ids.insert(endpoint.stable_id.to_ascii_lowercase()) {
            return Err(format!(
                "managed virtual endpoint `{}` is configured more than once",
                endpoint.stable_id
            ));
        }
        if !ports.insert(endpoint.com_port.to_ascii_uppercase()) {
            return Err(format!(
                "managed virtual COM port `{}` is configured more than once",
                endpoint.com_port
            ));
        }
    }
    Ok(desired)
}

fn desired_endpoint(
    stable_id: &str,
    kind: EndpointKind,
    configured_name: &str,
    requested_port: Option<&str>,
) -> Result<DesiredEndpoint, String> {
    let stable_id = stable_id.trim().to_ascii_lowercase();
    let definition = definition(&stable_id)
        .ok_or_else(|| format!("unknown CatHub virtual endpoint `{stable_id}`"))?;
    if definition.kind != kind {
        return Err(format!(
            "virtual endpoint `{stable_id}` is a {:?} endpoint and cannot back this {:?} configuration",
            definition.kind, kind
        ));
    }
    let com_port = normalize_com_port(requested_port.unwrap_or(definition.default_port))?;
    Ok(DesiredEndpoint {
        stable_id,
        kind,
        hardware_id: definition.hardware_id.to_string(),
        display_name: definition.display_name.to_string(),
        com_port,
        configured_name: configured_name.to_string(),
    })
}

fn definition(stable_id: &str) -> Option<&'static EndpointDefinition> {
    ENDPOINTS
        .iter()
        .find(|endpoint| endpoint.stable_id.eq_ignore_ascii_case(stable_id))
}

fn definition_for_hardware_id(hardware_id: &str) -> Option<&'static EndpointDefinition> {
    ENDPOINTS
        .iter()
        .find(|endpoint| endpoint.hardware_id.eq_ignore_ascii_case(hardware_id))
}

fn normalize_com_port(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_uppercase();
    let number = value
        .strip_prefix("COM")
        .and_then(|number| number.parse::<u16>().ok())
        .filter(|number| (1..=4096).contains(number))
        .ok_or_else(|| format!("`{value}` is not a valid COM1 through COM4096 name"))?;
    Ok(format!("COM{number}"))
}

fn plan_from_snapshot(
    mut desired: Vec<DesiredEndpoint>,
    snapshot: &SystemSnapshot,
) -> Result<ProvisionPlan, String> {
    desired.sort_by(|left, right| left.stable_id.cmp(&right.stable_id));
    let installed = snapshot
        .owned
        .iter()
        .map(|endpoint| (endpoint.stable_id.to_ascii_lowercase(), endpoint))
        .collect::<BTreeMap<_, _>>();
    let desired_ids = desired
        .iter()
        .map(|endpoint| endpoint.stable_id.as_str())
        .collect::<BTreeSet<_>>();
    let claims = snapshot
        .claims
        .iter()
        .map(|claim| (claim.com_port.to_ascii_uppercase(), claim))
        .collect::<BTreeMap<_, _>>();
    let mut actions = Vec::new();
    let mut conflicts = Vec::new();

    for endpoint in &desired {
        let installed_endpoint = installed.get(&endpoint.stable_id);
        if let Some(claim) = claims.get(&endpoint.com_port) {
            let claim_belongs_to_endpoint = installed_endpoint.is_some_and(|installed| {
                installed.instance_id.eq_ignore_ascii_case(&claim.owner)
                    || installed
                        .com_port
                        .as_ref()
                        .is_some_and(|port| port.eq_ignore_ascii_case(&claim.com_port))
            });
            if !claim_belongs_to_endpoint {
                conflicts.push((*claim).clone());
            }
        }
        match installed_endpoint {
            None => actions.push(ProvisionAction::Create {
                stable_id: endpoint.stable_id.clone(),
                hardware_id: endpoint.hardware_id.clone(),
                com_port: endpoint.com_port.clone(),
            }),
            Some(installed_endpoint)
                if installed_endpoint
                    .com_port
                    .as_ref()
                    .is_some_and(|port| port.eq_ignore_ascii_case(&endpoint.com_port))
                    && installed_endpoint.authorized_for_current_user =>
            {
                actions.push(ProvisionAction::Retain {
                    stable_id: endpoint.stable_id.clone(),
                    instance_id: installed_endpoint.instance_id.clone(),
                    com_port: endpoint.com_port.clone(),
                });
            }
            Some(installed_endpoint)
                if installed_endpoint
                    .com_port
                    .as_ref()
                    .is_some_and(|port| port.eq_ignore_ascii_case(&endpoint.com_port)) =>
            {
                actions.push(ProvisionAction::Authorize {
                    stable_id: endpoint.stable_id.clone(),
                    instance_id: installed_endpoint.instance_id.clone(),
                    com_port: endpoint.com_port.clone(),
                });
            }
            Some(installed_endpoint) => actions.push(ProvisionAction::Reassign {
                stable_id: endpoint.stable_id.clone(),
                instance_id: installed_endpoint.instance_id.clone(),
                from: installed_endpoint.com_port.clone(),
                to: endpoint.com_port.clone(),
            }),
        }
    }
    conflicts.sort_by(|left, right| left.com_port.cmp(&right.com_port));
    conflicts.dedup_by(|left, right| left.com_port.eq_ignore_ascii_case(&right.com_port));

    let duplicate_installed = snapshot.owned.iter().find(|endpoint| {
        desired_ids.contains(endpoint.stable_id.as_str())
            && snapshot
                .owned
                .iter()
                .filter(|candidate| candidate.stable_id == endpoint.stable_id)
                .count()
                > 1
    });
    if let Some(endpoint) = duplicate_installed {
        return Err(format!(
            "multiple CatHub-owned PnP devices advertise stable endpoint `{}`; remove the duplicates before applying",
            endpoint.stable_id
        ));
    }

    Ok(ProvisionPlan {
        desired,
        actions,
        conflicts,
    })
}

fn format_conflicts(conflicts: &[PortClaim]) -> String {
    let details = conflicts
        .iter()
        .map(|claim| format!("{} ({})", claim.com_port, claim.owner))
        .collect::<Vec<_>>()
        .join(", ");
    format!("requested COM ports are already claimed: {details}")
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use std::collections::BTreeSet;
    use std::ffi::c_void;
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DiInstallDriverW, SetupDiCallClassInstaller, SetupDiCreateDeviceInfoList,
        SetupDiCreateDeviceInfoW, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
        SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW,
        SetupDiOpenDevRegKey, SetupDiOpenDeviceInfoW, SetupDiRemoveDevice, SetupDiRestartDevices,
        SetupDiSetDeviceRegistryPropertyW, UpdateDriverForPlugAndPlayDevicesW, DICD_GENERATE_ID,
        DICS_FLAG_GLOBAL, DIF_REGISTERDEVICE, DIGCF_ALLCLASSES, DIIRFLAG_FORCE_INF, DIREG_DEV,
        GUID_DEVCLASS_PORTS, HDEVINFO, INSTALLFLAG_FORCE, SPDRP_DEVICEDESC, SPDRP_FRIENDLYNAME,
        SPDRP_HARDWAREID, SP_DEVINFO_DATA,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::{
        GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_SZ,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows_sys::Win32::UI::Shell::IsUserAnAdmin;

    use super::{
        definition_for_hardware_id, normalize_com_port, InstalledEndpoint, PortClaim,
        ProvisionAction, ProvisionPlan, SystemSnapshot,
    };

    const COM_NAME_ARBITER: &str = r"SYSTEM\CurrentControlSet\Control\COM Name Arbiter";
    const COM_DATABASE_VALUE: &str = "ComDB";
    const PORT_NAME_VALUE: &str = "PortName";
    const OWNER_SID_VALUE: &str = "CatHubOwnerSid";
    const MAX_COM_PORTS: usize = 4096;

    type HComDb = *mut c_void;

    #[link(name = "msports")]
    unsafe extern "system" {
        fn ComDBOpen(database: *mut HComDb) -> i32;
        fn ComDBClose(database: HComDb) -> i32;
        fn ComDBClaimPort(
            database: HComDb,
            com_number: u32,
            force_claim: i32,
            forced: *mut i32,
        ) -> i32;
        fn ComDBReleasePort(database: HComDb, com_number: u32) -> i32;
    }

    struct DeviceInfoSet(HDEVINFO);

    impl DeviceInfoSet {
        fn all() -> Result<Self, String> {
            // SAFETY: Null class/enumerator selects all device classes; no window owner is needed.
            let handle =
                unsafe { SetupDiGetClassDevsW(null(), null(), null_mut(), DIGCF_ALLCLASSES) };
            if handle == INVALID_HANDLE_VALUE as HDEVINFO {
                Err(last_error("enumerating Windows PnP devices"))
            } else {
                Ok(Self(handle))
            }
        }

        fn ports() -> Result<Self, String> {
            // SAFETY: The class GUID is static and no window owner is needed.
            let handle = unsafe { SetupDiCreateDeviceInfoList(&GUID_DEVCLASS_PORTS, null_mut()) };
            if handle == INVALID_HANDLE_VALUE as HDEVINFO {
                Err(last_error("creating a Ports-class device information set"))
            } else {
                Ok(Self(handle))
            }
        }
    }

    impl Drop for DeviceInfoSet {
        fn drop(&mut self) {
            // SAFETY: This wrapper exclusively owns the valid SetupAPI information-set handle.
            unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }

    struct ComDatabase(HComDb);

    impl ComDatabase {
        fn open() -> Result<Self, String> {
            let mut database = null_mut();
            // SAFETY: The output pointer is valid and receives an opaque handle on success.
            let result = unsafe { ComDBOpen(&raw mut database) };
            if result == 0 {
                Ok(Self(database))
            } else {
                Err(format!(
                    "opening the COM Name Arbiter database failed with Win32 error {result}"
                ))
            }
        }

        fn claim(&self, com_port: &str) -> Result<(), String> {
            let number = com_number(com_port)?;
            let mut forced = 0;
            // SAFETY: The database handle is open and the output pointer is valid.
            let result = unsafe { ComDBClaimPort(self.0, number, 0, &raw mut forced) };
            if result == 0 {
                Ok(())
            } else {
                Err(format!(
                    "claiming {com_port} in the COM Name Arbiter failed with Win32 error {result}"
                ))
            }
        }

        fn release(&self, com_port: &str) -> Result<(), String> {
            let number = com_number(com_port)?;
            // SAFETY: The database handle is open and the port number is within the documented range.
            let result = unsafe { ComDBReleasePort(self.0, number) };
            if result == 0 {
                Ok(())
            } else {
                Err(format!(
                    "releasing {com_port} in the COM Name Arbiter failed with Win32 error {result}"
                ))
            }
        }
    }

    impl Drop for ComDatabase {
        fn drop(&mut self) {
            // SAFETY: This wrapper exclusively owns the open COM database handle.
            unsafe { ComDBClose(self.0) };
        }
    }

    pub(super) fn snapshot() -> Result<SystemSnapshot, String> {
        let set = DeviceInfoSet::all()?;
        let current_sid = current_user_sid()?;
        let mut owned = Vec::new();
        let mut claims = Vec::new();
        let mut live_ports = BTreeSet::new();
        let mut index = 0;
        loop {
            let mut data = device_info_data();
            // SAFETY: The information set is valid and the initialized output remains live.
            if unsafe { SetupDiEnumDeviceInfo(set.0, index, &raw mut data) } == 0 {
                // SAFETY: GetLastError immediately follows the failed SetupAPI call.
                let error = unsafe { GetLastError() };
                if error == ERROR_NO_MORE_ITEMS {
                    break;
                }
                return Err(last_error("enumerating a Windows PnP device"));
            }
            index += 1;
            let instance_id = device_instance_id(set.0, &data)?;
            let port =
                device_port_name(set.0, &data).and_then(|port| normalize_com_port(&port).ok());
            let hardware_ids = device_property_strings(set.0, &data, SPDRP_HARDWAREID);
            let definition = hardware_ids
                .iter()
                .find_map(|hardware_id| definition_for_hardware_id(hardware_id));
            if let Some(port) = &port {
                live_ports.insert(port.clone());
                claims.push(PortClaim {
                    com_port: port.clone(),
                    owner: instance_id.clone(),
                    cathub_owned: definition.is_some(),
                });
            }
            if let Some(definition) = definition {
                let owner_sid = device_binary_value(set.0, &data, OWNER_SID_VALUE);
                let display_name = device_property_strings(set.0, &data, SPDRP_FRIENDLYNAME)
                    .into_iter()
                    .next()
                    .or_else(|| {
                        device_property_strings(set.0, &data, SPDRP_DEVICEDESC)
                            .into_iter()
                            .next()
                    })
                    .unwrap_or_else(|| definition.display_name.to_string());
                owned.push(InstalledEndpoint {
                    stable_id: definition.stable_id.to_string(),
                    kind: definition.kind,
                    hardware_id: definition.hardware_id.to_string(),
                    instance_id,
                    display_name,
                    com_port: port,
                    authorized_for_current_user: owner_sid.as_deref()
                        == Some(current_sid.as_slice()),
                });
            }
        }
        for port in arbiter_claims()? {
            if live_ports.insert(port.clone()) {
                claims.push(PortClaim {
                    com_port: port,
                    owner: "COM Name Arbiter reservation".to_string(),
                    cathub_owned: false,
                });
            }
        }
        owned.sort_by(|left, right| left.stable_id.cmp(&right.stable_id));
        claims.sort_by(|left, right| com_number(&left.com_port).cmp(&com_number(&right.com_port)));
        Ok(SystemSnapshot { owned, claims })
    }

    pub(super) fn apply(plan: &ProvisionPlan, inf_path: &Path) -> Result<bool, String> {
        require_administrator()?;
        let inf = wide(&inf_path.to_string_lossy());
        let mut reboot_required = 0;
        // SAFETY: The canonical path is NUL-terminated and all outputs are valid for the call.
        if unsafe {
            DiInstallDriverW(
                null_mut(),
                inf.as_ptr(),
                DIIRFLAG_FORCE_INF,
                &raw mut reboot_required,
            )
        } == 0
        {
            return Err(last_error(&format!(
                "staging CatHub driver `{}`",
                inf_path.display()
            )));
        }
        let database = ComDatabase::open()?;
        let owner_sid = current_user_sid()?;
        for action in &plan.actions {
            match action {
                ProvisionAction::Create {
                    stable_id,
                    hardware_id,
                    com_port,
                } => {
                    let definition = definition_for_hardware_id(hardware_id).ok_or_else(|| {
                        format!("refusing to create non-CatHub hardware ID `{hardware_id}`")
                    })?;
                    if definition.stable_id != stable_id {
                        return Err(format!("stable endpoint `{stable_id}` does not own hardware ID `{hardware_id}`"));
                    }
                    database.claim(com_port)?;
                    if let Err(error) = create_device(
                        definition.hardware_id,
                        definition.display_name,
                        com_port,
                        &owner_sid,
                        inf_path,
                        &mut reboot_required,
                    ) {
                        let _ = database.release(com_port);
                        return Err(error);
                    }
                }
                ProvisionAction::Reassign {
                    instance_id,
                    from,
                    to,
                    ..
                } => {
                    database.claim(to)?;
                    if let Err(error) = set_existing_port(instance_id, to, &owner_sid) {
                        let _ = database.release(to);
                        return Err(error);
                    }
                    if let Some(from) = from {
                        database.release(from)?;
                    }
                }
                ProvisionAction::Authorize { instance_id, .. } => {
                    set_existing_owner(instance_id, &owner_sid)?;
                }
                ProvisionAction::Retain { .. } => {}
            }
        }
        Ok(reboot_required != 0)
    }

    pub(super) fn remove(endpoints: &[InstalledEndpoint]) -> Result<bool, String> {
        require_administrator()?;
        let database = ComDatabase::open()?;
        let mut reboot_required = false;
        for endpoint in endpoints {
            let definition =
                definition_for_hardware_id(&endpoint.hardware_id).ok_or_else(|| {
                    format!(
                        "refusing to remove non-CatHub device `{}`",
                        endpoint.instance_id
                    )
                })?;
            if definition.stable_id != endpoint.stable_id {
                return Err(format!(
                    "device `{}` has inconsistent CatHub ownership metadata",
                    endpoint.instance_id
                ));
            }
            with_device(&endpoint.instance_id, |set, data| {
                // SAFETY: The device data belongs to the live information set and was found by exact instance ID.
                if unsafe { SetupDiRemoveDevice(set, data) } == 0 {
                    return Err(last_error(&format!(
                        "removing PnP device `{}`",
                        endpoint.instance_id
                    )));
                }
                Ok(())
            })?;
            if let Some(port) = &endpoint.com_port {
                database.release(port)?;
            }
            reboot_required |= false;
        }
        Ok(reboot_required)
    }

    fn create_device(
        hardware_id: &str,
        display_name: &str,
        com_port: &str,
        owner_sid: &[u8],
        inf_path: &Path,
        reboot_required: &mut i32,
    ) -> Result<(), String> {
        let set = DeviceInfoSet::ports()?;
        let class_name = wide("Ports");
        let description = wide(display_name);
        let mut data = device_info_data();
        // SAFETY: All strings and structures remain valid throughout this synchronous call.
        if unsafe {
            SetupDiCreateDeviceInfoW(
                set.0,
                class_name.as_ptr(),
                &GUID_DEVCLASS_PORTS,
                description.as_ptr(),
                null_mut(),
                DICD_GENERATE_ID,
                &raw mut data,
            )
        } == 0
        {
            return Err(last_error(&format!(
                "creating CatHub device `{hardware_id}`"
            )));
        }
        let mut hardware_id_value = wide(hardware_id);
        hardware_id_value.push(0);
        let byte_len = byte_len(&hardware_id_value)?;
        // SAFETY: The buffer is a valid NUL-terminated REG_MULTI_SZ and the device data belongs to the set.
        if unsafe {
            SetupDiSetDeviceRegistryPropertyW(
                set.0,
                &raw mut data,
                SPDRP_HARDWAREID,
                hardware_id_value.as_ptr().cast(),
                byte_len,
            )
        } == 0
        {
            return Err(last_error(&format!("setting hardware ID `{hardware_id}`")));
        }
        // SAFETY: Registering the newly created device uses its owning information set.
        if unsafe { SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, &raw const data) } == 0 {
            return Err(last_error(&format!(
                "registering CatHub device `{hardware_id}`"
            )));
        }
        let result = (|| {
            set_port_name(set.0, &data, com_port)?;
            set_owner_sid(set.0, &data, owner_sid)?;
            let hardware_id = wide(hardware_id);
            let inf = wide(&inf_path.to_string_lossy());
            // SAFETY: Both input strings are NUL-terminated and the reboot output is valid.
            if unsafe {
                UpdateDriverForPlugAndPlayDevicesW(
                    null_mut(),
                    hardware_id.as_ptr(),
                    inf.as_ptr(),
                    INSTALLFLAG_FORCE,
                    reboot_required,
                )
            } == 0
            {
                return Err(last_error(&format!(
                    "installing the CatHub driver for `{hardware_id:?}`"
                )));
            }
            Ok(())
        })();
        if result.is_err() {
            // SAFETY: Best-effort rollback targets only the just-created CatHub device.
            unsafe { SetupDiRemoveDevice(set.0, &raw mut data) };
        }
        result
    }

    fn set_existing_port(
        instance_id: &str,
        com_port: &str,
        owner_sid: &[u8],
    ) -> Result<(), String> {
        with_device(instance_id, |set, data| {
            set_port_name(set, data, com_port)?;
            set_owner_sid(set, data, owner_sid)?;
            // SAFETY: The data belongs to the live set and identifies the exact CatHub instance.
            if unsafe { SetupDiRestartDevices(set, data) } == 0 {
                return Err(last_error(&format!(
                    "restarting CatHub device `{instance_id}`"
                )));
            }
            Ok(())
        })
    }

    fn set_existing_owner(instance_id: &str, owner_sid: &[u8]) -> Result<(), String> {
        with_device(instance_id, |set, data| {
            set_owner_sid(set, data, owner_sid)?;
            // SAFETY: Restart makes the driver reload the updated owner SID.
            if unsafe { SetupDiRestartDevices(set, data) } == 0 {
                return Err(last_error(&format!(
                    "restarting CatHub device `{instance_id}`"
                )));
            }
            Ok(())
        })
    }

    fn with_device(
        instance_id: &str,
        operation: impl FnOnce(HDEVINFO, &mut SP_DEVINFO_DATA) -> Result<(), String>,
    ) -> Result<(), String> {
        let set = DeviceInfoSet::all()?;
        let instance = wide(instance_id);
        let mut data = device_info_data();
        // SAFETY: The exact instance ID is NUL-terminated and output storage is initialized.
        if unsafe { SetupDiOpenDeviceInfoW(set.0, instance.as_ptr(), null_mut(), 0, &raw mut data) }
            == 0
        {
            return Err(last_error(&format!("opening PnP device `{instance_id}`")));
        }
        operation(set.0, &mut data)
    }

    fn set_port_name(set: HDEVINFO, data: &SP_DEVINFO_DATA, com_port: &str) -> Result<(), String> {
        // SAFETY: The device data belongs to the live information set.
        let key = unsafe {
            SetupDiOpenDevRegKey(set, data, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_SET_VALUE)
        };
        if key as isize == INVALID_HANDLE_VALUE as isize {
            return Err(last_error(&format!(
                "opening the device registry key for {com_port}"
            )));
        }
        let name = wide(PORT_NAME_VALUE);
        let value = wide(com_port);
        let result = byte_len(&value).and_then(|length| {
            // SAFETY: The key is open and both strings remain valid through the call.
            let status = unsafe {
                RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr().cast(), length)
            };
            if status == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!(
                    "setting PortName={com_port} failed with Win32 error {status}"
                ))
            }
        });
        // SAFETY: This function owns the registry handle returned above.
        unsafe { RegCloseKey(key) };
        result
    }

    fn set_owner_sid(
        set: HDEVINFO,
        data: &SP_DEVINFO_DATA,
        owner_sid: &[u8],
    ) -> Result<(), String> {
        // SAFETY: The device data belongs to the live information set.
        let key = unsafe {
            SetupDiOpenDevRegKey(set, data, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_SET_VALUE)
        };
        if key as isize == INVALID_HANDLE_VALUE as isize {
            return Err(last_error(
                "opening the CatHub device security registry key",
            ));
        }
        let name = wide(OWNER_SID_VALUE);
        let length = u32::try_from(owner_sid.len())
            .map_err(|_| "the current user's SID is too large".to_string());
        let result = length.and_then(|length| {
            // SAFETY: The key is open and the copied SID bytes remain valid through the call.
            let status = unsafe {
                RegSetValueExW(
                    key,
                    name.as_ptr(),
                    0,
                    REG_BINARY,
                    owner_sid.as_ptr(),
                    length,
                )
            };
            if status == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!(
                    "setting CatHubOwnerSid failed with Win32 error {status}"
                ))
            }
        });
        // SAFETY: This function owns the registry handle returned above.
        unsafe { RegCloseKey(key) };
        result
    }

    fn device_port_name(set: HDEVINFO, data: &SP_DEVINFO_DATA) -> Option<String> {
        // SAFETY: The device data belongs to the live information set.
        let key =
            unsafe { SetupDiOpenDevRegKey(set, data, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ) };
        if key as isize == INVALID_HANDLE_VALUE as isize {
            return None;
        }
        let value = query_registry_string(key, PORT_NAME_VALUE);
        // SAFETY: This function owns the registry handle returned above.
        unsafe { RegCloseKey(key) };
        value
    }

    fn device_binary_value(set: HDEVINFO, data: &SP_DEVINFO_DATA, name: &str) -> Option<Vec<u8>> {
        // SAFETY: The device data belongs to the live information set.
        let key =
            unsafe { SetupDiOpenDevRegKey(set, data, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ) };
        if key as isize == INVALID_HANDLE_VALUE as isize {
            return None;
        }
        let name = wide(name);
        let mut kind = 0;
        let mut bytes = 0;
        // SAFETY: Null data performs a size query on the open key.
        let status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                null(),
                &raw mut kind,
                null_mut(),
                &raw mut bytes,
            )
        };
        let mut value = if status == ERROR_SUCCESS && kind == REG_BINARY && bytes > 0 {
            vec![0_u8; usize::try_from(bytes).ok()?]
        } else {
            Vec::new()
        };
        if !value.is_empty() {
            // SAFETY: The byte buffer matches the registry-reported size.
            let status = unsafe {
                RegQueryValueExW(
                    key,
                    name.as_ptr(),
                    null(),
                    &raw mut kind,
                    value.as_mut_ptr(),
                    &raw mut bytes,
                )
            };
            if status != ERROR_SUCCESS {
                value.clear();
            }
        }
        // SAFETY: This function owns the registry handle returned above.
        unsafe { RegCloseKey(key) };
        (!value.is_empty()).then_some(value)
    }

    fn query_registry_string(key: HKEY, name: &str) -> Option<String> {
        let name = wide(name);
        let mut kind = 0;
        let mut bytes = 0;
        // SAFETY: The key is open and size output is valid; null data performs a size query.
        if unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                null(),
                &raw mut kind,
                null_mut(),
                &raw mut bytes,
            )
        } != ERROR_SUCCESS
            || kind != REG_SZ
            || bytes == 0
        {
            return None;
        }
        let mut buffer = vec![0_u16; usize::try_from(bytes).ok()?.div_ceil(2)];
        // SAFETY: The byte-sized buffer and all outputs remain valid for the query.
        if unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                null(),
                &raw mut kind,
                buffer.as_mut_ptr().cast(),
                &raw mut bytes,
            )
        } != ERROR_SUCCESS
        {
            return None;
        }
        Some(
            String::from_utf16_lossy(&buffer)
                .trim_matches('\0')
                .trim()
                .to_string(),
        )
    }

    fn arbiter_claims() -> Result<Vec<String>, String> {
        let path = wide(COM_NAME_ARBITER);
        let mut key = null_mut();
        // SAFETY: The path is NUL-terminated and output storage is valid.
        let status =
            unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &raw mut key) };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "opening the COM Name Arbiter failed with Win32 error {status}"
            ));
        }
        let name = wide(COM_DATABASE_VALUE);
        let mut kind = 0;
        let mut bytes = 0;
        // SAFETY: The key is open and null data requests the required byte count.
        let size_status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                null(),
                &raw mut kind,
                null_mut(),
                &raw mut bytes,
            )
        };
        if size_status != ERROR_SUCCESS || kind != REG_BINARY {
            // SAFETY: This function owns the open key.
            unsafe { RegCloseKey(key) };
            return Err(format!(
                "reading the COM Name Arbiter database failed with Win32 error {size_status}"
            ));
        }
        let mut bitmap =
            vec![0_u8; usize::try_from(bytes).map_err(|_| "COM database is too large")?];
        // SAFETY: The allocated byte buffer matches the requested registry value size.
        let query_status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                null(),
                &raw mut kind,
                bitmap.as_mut_ptr(),
                &raw mut bytes,
            )
        };
        // SAFETY: This function owns the open key.
        unsafe { RegCloseKey(key) };
        if query_status != ERROR_SUCCESS {
            return Err(format!(
                "reading the COM Name Arbiter database failed with Win32 error {query_status}"
            ));
        }
        let mut claims = Vec::new();
        for number in 1..=MAX_COM_PORTS.min(bitmap.len() * 8) {
            let bit = number - 1;
            if bitmap
                .get(bit / 8)
                .is_some_and(|value| value & (1 << (bit % 8)) != 0)
            {
                claims.push(format!("COM{number}"));
            }
        }
        Ok(claims)
    }

    fn device_property_strings(
        set: HDEVINFO,
        data: &SP_DEVINFO_DATA,
        property: u32,
    ) -> Vec<String> {
        let mut kind = 0;
        let mut bytes = 0;
        // SAFETY: Null data performs a size query for a valid device and property.
        unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                set,
                data,
                property,
                &raw mut kind,
                null_mut(),
                0,
                &raw mut bytes,
            );
        }
        if bytes == 0 {
            return Vec::new();
        }
        let Ok(units) = usize::try_from(bytes).map(|bytes| bytes.div_ceil(2)) else {
            return Vec::new();
        };
        let mut buffer = vec![0_u16; units];
        // SAFETY: The byte-sized buffer matches the size reported by SetupAPI.
        if unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                set,
                data,
                property,
                &raw mut kind,
                buffer.as_mut_ptr().cast(),
                bytes,
                &raw mut bytes,
            )
        } == 0
        {
            return Vec::new();
        }
        buffer
            .split(|unit| *unit == 0)
            .filter(|value| !value.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }

    fn device_instance_id(set: HDEVINFO, data: &SP_DEVINFO_DATA) -> Result<String, String> {
        let mut required = 0;
        // SAFETY: Null output requests the required UTF-16 unit count.
        unsafe { SetupDiGetDeviceInstanceIdW(set, data, null_mut(), 0, &raw mut required) };
        if required == 0 {
            return Err(last_error("querying a PnP device instance ID length"));
        }
        let mut buffer =
            vec![0_u16; usize::try_from(required).map_err(|_| "PnP instance ID is too long")?];
        // SAFETY: The buffer has the exact unit count reported by SetupAPI.
        if unsafe {
            SetupDiGetDeviceInstanceIdW(set, data, buffer.as_mut_ptr(), required, &raw mut required)
        } == 0
        {
            return Err(last_error("reading a PnP device instance ID"));
        }
        Ok(String::from_utf16_lossy(&buffer)
            .trim_matches('\0')
            .to_string())
    }

    fn current_user_sid() -> Result<Vec<u8>, String> {
        let mut token = null_mut();
        // SAFETY: The pseudo-process handle is valid and output storage receives an owned token.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(last_error("opening the current process token"));
        }
        let result = (|| {
            let mut bytes = 0;
            // SAFETY: Null output requests the token-user buffer size.
            unsafe {
                GetTokenInformation(token, TokenUser, null_mut(), 0, &raw mut bytes);
            }
            // SAFETY: GetLastError immediately follows the expected size-query failure.
            if bytes == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
                return Err(last_error("querying the current user SID length"));
            }
            let mut buffer = vec![
                0_u8;
                usize::try_from(bytes)
                    .map_err(|_| "the current user token is too large")?
            ];
            // SAFETY: The buffer has the exact size requested by GetTokenInformation.
            if unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    bytes,
                    &raw mut bytes,
                )
            } == 0
            {
                return Err(last_error("reading the current user SID"));
            }
            // SAFETY: TOKEN_USER is the documented leading structure in this returned buffer.
            let token_user = unsafe { (buffer.as_ptr().cast::<TOKEN_USER>()).read_unaligned() };
            // SAFETY: The SID pointer belongs to the live token-information buffer.
            let sid_length = unsafe { GetLengthSid(token_user.User.Sid) };
            let sid_length =
                usize::try_from(sid_length).map_err(|_| "the current user SID is too large")?;
            if sid_length == 0 {
                return Err(last_error("validating the current user SID"));
            }
            // SAFETY: GetLengthSid returned the byte length of this SID in the live buffer.
            Ok(unsafe {
                std::slice::from_raw_parts(token_user.User.Sid.cast::<u8>(), sid_length).to_vec()
            })
        })();
        // SAFETY: This function owns the process token handle.
        unsafe { CloseHandle(token) };
        result
    }

    fn require_administrator() -> Result<(), String> {
        // SAFETY: This parameterless shell helper checks membership in the local Administrators group.
        if unsafe { IsUserAnAdmin() } == 0 {
            Err("virtual serial apply/remove requires an elevated Administrator shell".to_string())
        } else {
            Ok(())
        }
    }

    fn device_info_data() -> SP_DEVINFO_DATA {
        // SAFETY: The Windows structure is plain-old-data and requires cbSize initialization.
        let mut data: SP_DEVINFO_DATA = unsafe { zeroed() };
        data.cbSize = u32::try_from(size_of::<SP_DEVINFO_DATA>()).unwrap_or(u32::MAX);
        data
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn byte_len(value: &[u16]) -> Result<u32, String> {
        u32::try_from(value.len().saturating_mul(size_of::<u16>()))
            .map_err(|_| "UTF-16 registry value is too large".to_string())
    }

    fn com_number(com_port: &str) -> Result<u32, String> {
        normalize_com_port(com_port)?
            .strip_prefix("COM")
            .and_then(|number| number.parse().ok())
            .ok_or_else(|| format!("invalid COM port `{com_port}`"))
    }

    fn last_error(context: &str) -> String {
        format!("{context}: {}", io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{InstalledEndpoint, ProvisionPlan, SystemSnapshot};
    use std::path::Path;

    pub(super) fn snapshot() -> Result<SystemSnapshot, String> {
        Err("CatHub virtual serial provisioning requires Windows".to_string())
    }

    pub(super) fn apply(_plan: &ProvisionPlan, _inf_path: &Path) -> Result<bool, String> {
        Err("CatHub virtual serial provisioning requires Windows".to_string())
    }

    pub(super) fn remove(_endpoints: &[InstalledEndpoint]) -> Result<bool, String> {
        Err("CatHub virtual serial provisioning requires Windows".to_string())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn config(body: &str) -> Config {
        Config::parse(body).expect("config")
    }

    #[test]
    fn derives_the_five_production_endpoint_identities() {
        let config = config(
            r#"
[radio]
backend = "loopback"

[[serial_endpoint]]
name = "hdsdr"
virtual_endpoint = "hdsdr-cat"
application_transport = "COM11"
dialect = "ts2000"

[[serial_endpoint]]
name = "n1mm"
virtual_endpoint = "n1mm-cat"
application_transport = "COM21"
dialect = "ts590"

[[serial_endpoint]]
name = "arcp"
virtual_endpoint = "arcp590-cat"
application_transport = "COM31"
dialect = "ts590-transparent"

[winkeyer]
port = "COM3"

[[winkeyer_endpoint]]
name = "n1mm-winkeyer"
virtual_endpoint = "n1mm-winkeyer"
application_transport = "COM41"

[[winkeyer_endpoint]]
name = "wktools"
virtual_endpoint = "wktools"
application_transport = "COM43"
"#,
        );
        let desired = desired_endpoints(&config).expect("desired");
        assert_eq!(desired.len(), 5);
        assert_eq!(
            desired.first().unwrap().hardware_id,
            r"ROOT\CATHUB_HDSDR_CAT"
        );
        assert_eq!(desired.get(4).unwrap().com_port, "COM43");
    }

    #[test]
    fn detects_non_cathub_com_claims() {
        let config = config(
            r#"
[radio]
backend = "loopback"
[[serial_endpoint]]
name = "n1mm"
virtual_endpoint = "n1mm-cat"
application_transport = "com21"
dialect = "ts590"
"#,
        );
        let snapshot = SystemSnapshot {
            owned: Vec::new(),
            claims: vec![PortClaim {
                com_port: "COM21".to_string(),
                owner: "USB\\VID_1234".to_string(),
                cathub_owned: false,
            }],
        };
        let plan = plan_from_snapshot(desired_endpoints(&config).unwrap(), &snapshot).unwrap();
        assert_eq!(plan.conflicts.len(), 1);
        assert!(matches!(
            plan.actions.first(),
            Some(ProvisionAction::Create { .. })
        ));
    }

    #[test]
    fn retained_endpoint_is_idempotent() {
        let config = config(
            r#"
[radio]
backend = "loopback"
[[serial_endpoint]]
name = "n1mm"
virtual_endpoint = "n1mm-cat"
application_transport = "COM21"
dialect = "ts590"
"#,
        );
        let snapshot = SystemSnapshot {
            owned: vec![InstalledEndpoint {
                stable_id: "n1mm-cat".to_string(),
                kind: EndpointKind::Cat,
                hardware_id: r"ROOT\CATHUB_N1MM_CAT".to_string(),
                instance_id: r"ROOT\CATHUB_N1MM_CAT\0000".to_string(),
                display_name: "CatHub N1MM CAT Port".to_string(),
                com_port: Some("COM21".to_string()),
                authorized_for_current_user: true,
            }],
            claims: vec![PortClaim {
                com_port: "COM21".to_string(),
                owner: r"ROOT\CATHUB_N1MM_CAT\0000".to_string(),
                cathub_owned: true,
            }],
        };
        let plan = plan_from_snapshot(desired_endpoints(&config).unwrap(), &snapshot).unwrap();
        assert!(plan.is_applicable());
        assert!(!plan.requires_changes());
    }

    #[test]
    fn installed_endpoint_with_another_owner_is_reauthorized() {
        let config = config(
            r#"
[radio]
backend = "loopback"
[[serial_endpoint]]
name = "n1mm"
virtual_endpoint = "n1mm-cat"
application_transport = "COM21"
dialect = "ts590"
"#,
        );
        let snapshot = SystemSnapshot {
            owned: vec![InstalledEndpoint {
                stable_id: "n1mm-cat".to_string(),
                kind: EndpointKind::Cat,
                hardware_id: r"ROOT\CATHUB_N1MM_CAT".to_string(),
                instance_id: r"ROOT\CATHUB_N1MM_CAT\0000".to_string(),
                display_name: "CatHub N1MM CAT Port".to_string(),
                com_port: Some("COM21".to_string()),
                authorized_for_current_user: false,
            }],
            claims: vec![PortClaim {
                com_port: "COM21".to_string(),
                owner: r"ROOT\CATHUB_N1MM_CAT\0000".to_string(),
                cathub_owned: true,
            }],
        };
        let plan = plan_from_snapshot(desired_endpoints(&config).unwrap(), &snapshot).unwrap();
        assert!(matches!(
            plan.actions.first(),
            Some(ProvisionAction::Authorize { .. })
        ));
        assert!(plan.requires_changes());
    }

    #[test]
    fn rejects_kind_mismatch_and_invalid_com_name() {
        assert!(desired_endpoint("wktools", EndpointKind::Cat, "bad", None).is_err());
        assert!(normalize_com_port("COM0").is_err());
        assert!(normalize_com_port("LPT1").is_err());
        assert_eq!(normalize_com_port("com0042").unwrap(), "COM42");
    }
}
