use thiserror::Error;

const MAX_EXTENSION_ID_BYTES: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExtensionCapability {
    Theme,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExtensionSourceKind {
    Embedded,
    DataFile,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ExtensionManifest<'a> {
    pub(crate) id: &'a str,
    pub(crate) version: u16,
    pub(crate) source: ExtensionSourceKind,
    pub(crate) capabilities: &'a [ExtensionCapability],
}

pub(crate) trait ExtensionAdapter {
    fn manifest(&self) -> ExtensionManifest<'_>;
}

#[derive(Debug, Error, Eq, PartialEq)]
pub(crate) enum ExtensionManifestError {
    #[error("extension ID is invalid")]
    InvalidId,
    #[error("extension version must be nonzero")]
    InvalidVersion,
    #[error("extension must declare at least one capability")]
    MissingCapabilities,
    #[error("extension declares a duplicate capability")]
    DuplicateCapability,
}

pub(crate) fn validate_manifest(
    manifest: ExtensionManifest<'_>,
) -> Result<(), ExtensionManifestError> {
    match manifest.source {
        ExtensionSourceKind::Embedded | ExtensionSourceKind::DataFile => {}
    }
    if manifest.id.is_empty()
        || manifest.id.len() > MAX_EXTENSION_ID_BYTES
        || !manifest
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
    {
        return Err(ExtensionManifestError::InvalidId);
    }
    if manifest.version == 0 {
        return Err(ExtensionManifestError::InvalidVersion);
    }
    if manifest.capabilities.is_empty() {
        return Err(ExtensionManifestError::MissingCapabilities);
    }
    for (index, capability) in manifest.capabilities.iter().enumerate() {
        if manifest.capabilities[..index].contains(capability) {
            return Err(ExtensionManifestError::DuplicateCapability);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_fail_closed_before_an_adapter_runs() {
        assert_eq!(
            validate_manifest(ExtensionManifest {
                id: "Unsafe Plugin",
                version: 1,
                source: ExtensionSourceKind::DataFile,
                capabilities: &[ExtensionCapability::Theme],
            }),
            Err(ExtensionManifestError::InvalidId)
        );
        assert_eq!(
            validate_manifest(ExtensionManifest {
                id: "theme.empty",
                version: 1,
                source: ExtensionSourceKind::DataFile,
                capabilities: &[],
            }),
            Err(ExtensionManifestError::MissingCapabilities)
        );
    }
}
