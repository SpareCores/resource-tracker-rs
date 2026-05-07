use crate::metrics::CloudInfo;
use serde::Deserialize;

use super::{imds_get, new_imds_agent};

#[derive(Debug, Deserialize)]
struct NetworkService {
    #[serde(rename = "type")]
    service_type: Option<String>,
    address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NetworkData {
    services: Option<Vec<NetworkService>>,
}

#[derive(Debug, Deserialize)]
struct MetaData {
    availability_zone: Option<String>,
}

/// OVH Public Cloud via OpenStack metadata endpoints.
///
/// Identifies OVH by checking that a DNS service entry in `network_data.json`
/// reports `213.186.33.99` (OVH's dedicated resolver). Region comes from
/// `availability_zone` in `meta_data.json`; instance type from the
/// EC2-compatible endpoint that OVH also exposes.
///
/// Reference: <https://docs.openstack.org/nova/latest/user/metadata.html>
pub fn probe() -> Option<CloudInfo> {
    let agent = new_imds_agent();

    // Confirm OVH fingerprint: DNS address in OpenStack network_data.json.
    let network_body = imds_get(
        &agent,
        "http://169.254.169.254/openstack/latest/network_data.json",
    )?;
    let network_data: NetworkData = serde_json::from_str(&network_body).ok()?;
    let is_ovh = network_data
        .services
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|s| {
            s.service_type.as_deref() == Some("dns")
                && s.address.as_deref() == Some("213.186.33.99")
        });
    if !is_ovh {
        return None;
    }

    // Region: availability_zone from OpenStack meta_data.json ("nova" is meaningless).
    let cloud_region_id = imds_get(
        &agent,
        "http://169.254.169.254/openstack/latest/meta_data.json",
    )
    .and_then(|body| serde_json::from_str::<MetaData>(&body).ok())
    .and_then(|m| m.availability_zone)
    .filter(|az| !az.is_empty() && az != "unknown" && az != "nova");

    // Instance type from the EC2-compatible endpoint OVH also exposes.
    let cloud_instance_type = imds_get(
        &agent,
        "http://169.254.169.254/latest/meta-data/instance-type",
    )
    .filter(|s| !s.is_empty() && s != "unknown");

    Some(CloudInfo {
        cloud_vendor_id: Some("ovh".to_string()),
        cloud_account_id: None,
        cloud_region_id,
        cloud_zone_id: None,
        cloud_instance_type,
    })
}
