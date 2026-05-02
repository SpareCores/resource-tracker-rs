use crate::metrics::CloudInfo;
use serde::Deserialize;

use super::new_imds_agent;

#[derive(Debug, Deserialize)]
struct InstanceMetadata {
    compute: Option<Compute>,
}

#[derive(Debug, Deserialize)]
struct Compute {
    #[serde(rename = "vmSize")]
    vm_size: Option<String>,
    location: Option<String>,
}

pub fn probe() -> Option<CloudInfo> {
    let agent = new_imds_agent();
    let url = "http://169.254.169.254/metadata/instance?api-version=2021-02-01";
    let body = agent
        .get(url)
        .header("Metadata", "true")
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())?;
    let meta: InstanceMetadata = serde_json::from_str(&body).ok()?;
    let compute = meta.compute?;

    Some(CloudInfo {
        cloud_vendor_id: Some("azure".to_string()),
        cloud_account_id: None,
        cloud_region_id: compute.location.filter(|s| !s.is_empty()),
        cloud_zone_id: None,
        cloud_instance_type: compute.vm_size.filter(|s| !s.is_empty()),
    })
}
