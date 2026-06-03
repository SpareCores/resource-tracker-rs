use crate::metrics::CloudInfo;
use serde::Deserialize;

use super::{imds_get, new_imds_agent};

#[derive(Debug, Deserialize)]
struct Region {
    regioncode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    instanceid: Option<String>,
    #[serde(rename = "instance-v2-id")]
    instance_v2_id: Option<String>,
    region: Option<Region>,
}

/// Vultr cloud metadata via the instance metadata service.
///
/// Reference: <https://www.vultr.com/metadata/>
pub fn probe() -> Option<CloudInfo> {
    let agent = new_imds_agent();
    let body = imds_get(&agent, "http://169.254.169.254/v1.json")?;
    let meta: Metadata = serde_json::from_str(&body).ok()?;

    if meta.instanceid.is_none() && meta.instance_v2_id.is_none() {
        return None;
    }

    Some(CloudInfo {
        cloud_vendor_id: Some("vultr".to_string()),
        cloud_account_id: None,
        cloud_region_id: meta
            .region
            .and_then(|r| r.regioncode)
            .filter(|s| !s.is_empty() && s != "unknown"),
        cloud_zone_id: None,
        cloud_instance_type: None,
    })
}
