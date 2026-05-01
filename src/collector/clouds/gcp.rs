use crate::metrics::CloudInfo;

use super::{imds_get_headers, new_imds_agent};

/// Derive the region from a GCP zone basename (e.g. `us-central1-a` → `us-central1`).
fn zone_to_region(zone: &str) -> String {
    match zone.rsplit_once('-') {
        Some((prefix, _)) => prefix.to_string(),
        None => zone.to_string(),
    }
}

pub fn probe() -> Option<CloudInfo> {
    let agent = new_imds_agent();
    const FLAVOR: &[(&str, &str)] = &[("Metadata-Flavor", "Google")];
    let machine_type = imds_get_headers(
        &agent,
        "http://metadata.google.internal/computeMetadata/v1/instance/machine-type",
        FLAVOR,
    )?;
    let zone_full = imds_get_headers(
        &agent,
        "http://metadata.google.internal/computeMetadata/v1/instance/zone",
        FLAVOR,
    )?;

    // projects/PROJECT_NUM/machineTypes/MACHINE_TYPE
    let instance_type = machine_type.rsplit('/').next()?.to_string();
    let zone = zone_full.rsplit('/').next()?.to_string();
    let cloud_region_id = zone_to_region(&zone);

    Some(CloudInfo {
        cloud_vendor_id: Some("gcp".to_string()),
        cloud_account_id: None,
        cloud_region_id: Some(cloud_region_id),
        cloud_zone_id: Some(zone),
        cloud_instance_type: Some(instance_type),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zone_to_region() {
        assert_eq!(zone_to_region("us-central1-a"), "us-central1");
        assert_eq!(zone_to_region("x"), "x");
        assert_eq!(zone_to_region("europe-west4-b"), "europe-west4");
    }
}
