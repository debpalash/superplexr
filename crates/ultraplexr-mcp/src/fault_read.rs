//! Compact metadata and consistency-pinned pages of existing Fault evidence.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ultraplexr_core::FaultId;
use ultraplexr_protocol::{FaultState, FaultSummary};

fn boolean(args: &Value, key: &str) -> Result<bool, String> {
    args.get(key).map_or(Ok(false), |value| {
        value
            .as_bool()
            .ok_or_else(|| format!("{key} must be a boolean"))
    })
}
fn integer(args: &Value, key: &str) -> Result<Option<u64>, String> {
    args.get(key)
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| format!("{key} must be a nonnegative integer"))
        })
        .transpose()
}
fn known_arguments(args: &Value, allowed: &[&str]) -> Result<(), String> {
    let object = args
        .as_object()
        .ok_or("Fault inspection arguments must be an object")?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("Unknown Fault inspection argument".into());
    }
    Ok(())
}
fn text(value: &str) -> Value {
    let mut end = value.len().min(256);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    json!({"text":&value[..end],"truncated":end < value.len(),"total_bytes":value.len()})
}
fn compact(fault: &FaultSummary) -> Value {
    let state = match fault.state {
        FaultState::Open => "open",
        FaultState::Resolved { .. } => "resolved",
        FaultState::Dismissed { .. } => "dismissed",
    };
    json!({"fault_id":fault.fault_id,"kind":fault.kind,"source":fault.source,"state":state,
        "summary":text(&fault.summary),"command":text(&fault.command),"cwd":text(&fault.cwd.to_string_lossy()),
        "exit_code":fault.exit_code,"observed_at_unix_micros":fault.observed_at_unix_micros.to_string(),
        "repro_attempts":fault.repro_attempts,"regressions":fault.regressions,
        "output_bytes":fault.output.len(),"repro_output_bytes":fault.repro.as_ref().map(|receipt| receipt.output.len()),
        "proof_output_bytes":fault.proof.as_ref().map(|receipt| receipt.output.len()),
        "latest_repro_passes":fault.repro_passes(),"compact":true,"details_omitted":true})
}

pub(super) struct ListOptions {
    compact: bool,
    limit: Option<usize>,
    after: Option<FaultId>,
}
impl ListOptions {
    pub(super) fn parse(args: &Value) -> Result<Self, String> {
        known_arguments(args, &["include_closed", "compact", "limit", "after"])?;
        let compact = boolean(args, "compact")?;
        let limit = integer(args, "limit")?
            .map(|limit| {
                if !(1..=200).contains(&limit) {
                    return Err("limit must be between 1 and 200".to_string());
                }
                Ok(limit as usize)
            })
            .transpose()?
            .or(compact.then_some(50));
        let after = args
            .get("after")
            .map(|value| {
                value
                    .as_str()
                    .ok_or("after must be a Fault ID string")?
                    .parse::<FaultId>()
                    .map_err(|_| "after must be a Fault ID string")
            })
            .transpose()?;
        if after.is_some() && limit.is_none() {
            return Err("after requires limit or compact mode".into());
        }
        Ok(Self {
            compact,
            limit,
            after,
        })
    }
    pub(super) fn project(self, faults: Vec<FaultSummary>) -> Result<Value, String> {
        let open = faults.iter().filter(|fault| fault.is_open()).count();
        if !self.compact && self.limit.is_none() {
            return Ok(json!({"faults":faults,"open":open}));
        }
        let start = self
            .after
            .map(|id| {
                faults
                    .iter()
                    .position(|fault| fault.fault_id == id)
                    .map(|index| index + 1)
                    .ok_or("Fault cursor is no longer in this list; restart without after")
            })
            .transpose()?
            .unwrap_or(0);
        let limit = self.limit.unwrap_or(50);
        let has_more = faults.len().saturating_sub(start) > limit;
        let page: Vec<_> = faults.into_iter().skip(start).take(limit).collect();
        let next = if has_more {
            page.last().map(|fault| fault.fault_id)
        } else {
            None
        };
        let values: Vec<_> = page
            .into_iter()
            .map(|fault| {
                if self.compact {
                    compact(&fault)
                } else {
                    json!(fault)
                }
            })
            .collect();
        Ok(
            json!({"faults":values,"open":open,"has_more":has_more,"next_after":next,"consistent_snapshot":false}),
        )
    }
}

pub(super) struct ShowOptions {
    compact: bool,
    field: Option<String>,
    offset: usize,
    limit: usize,
    expected: Option<String>,
}
impl ShowOptions {
    pub(super) fn parse(args: &Value) -> Result<Self, String> {
        known_arguments(
            args,
            &[
                "fault_id",
                "compact",
                "output_field",
                "offset",
                "max_bytes",
                "expected_sha256",
            ],
        )?;
        let compact = boolean(args, "compact")?;
        let field = args
            .get("output_field")
            .map(|value| {
                let field = value
                    .as_str()
                    .ok_or("output_field must be observed, repro, or proof")?;
                if !matches!(field, "observed" | "repro" | "proof") {
                    return Err("output_field must be observed, repro, or proof");
                }
                Ok(field.to_owned())
            })
            .transpose()?;
        let offset = integer(args, "offset")?.unwrap_or(0);
        let limit = integer(args, "max_bytes")?.unwrap_or(16_384);
        if !(4..=65_536).contains(&limit) {
            return Err("max_bytes must be between 4 and 65536".into());
        }
        let offset =
            usize::try_from(offset).map_err(|_| "offset exceeds this platform's address range")?;
        let expected = args
            .get("expected_sha256")
            .map(|value| {
                let digest = value
                    .as_str()
                    .ok_or("expected_sha256 must be 64 hexadecimal characters")?;
                if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err("expected_sha256 must be 64 hexadecimal characters");
                }
                Ok(digest.to_ascii_lowercase())
            })
            .transpose()?;
        if field.is_none()
            && ["offset", "max_bytes", "expected_sha256"]
                .iter()
                .any(|key| args.get(key).is_some())
        {
            return Err("Output paging arguments require output_field".into());
        }
        if field.is_some() && compact {
            return Err("Choose compact metadata or output_field, not both".into());
        }
        if offset > 0 && expected.is_none() {
            return Err(
                "Nonzero offset requires the previous page's sha256 as expected_sha256".into(),
            );
        }
        Ok(Self {
            compact,
            field,
            offset,
            limit: limit as usize,
            expected,
        })
    }
    pub(super) fn project(self, fault: FaultSummary) -> Result<Value, String> {
        let Some(field) = self.field else {
            return Ok(if self.compact {
                compact(&fault)
            } else {
                json!(fault)
            });
        };
        let (output, observed_at) = match field.as_str() {
            "observed" => (&fault.output, fault.observed_at_unix_micros),
            "repro" => {
                let receipt = fault
                    .repro
                    .as_ref()
                    .ok_or("No recorded replay output; inspection did not run a replay")?;
                (&receipt.output, receipt.attempted_at_unix_micros)
            }
            "proof" => {
                let receipt = fault
                    .proof
                    .as_ref()
                    .ok_or("No recorded proof output; inspection did not run a replay")?;
                (&receipt.output, receipt.attempted_at_unix_micros)
            }
            _ => return Err("Unsupported output field".into()),
        };
        let mut digest = Sha256::new();
        digest.update(b"ultraplexr-mcp-fault-output-v1\0");
        digest.update(fault.fault_id.to_string().as_bytes());
        digest.update(field.as_bytes());
        digest.update(observed_at.to_le_bytes());
        digest.update((output.len() as u64).to_le_bytes());
        digest.update(output.as_bytes());
        let digest = format!("{:x}", digest.finalize());
        if self
            .expected
            .as_ref()
            .is_some_and(|expected| expected != &digest)
        {
            return Err(
                "Recorded output changed; discard partial results and restart from offset 0".into(),
            );
        }
        if self.offset > output.len() || !output.is_char_boundary(self.offset) {
            return Err(
                "offset must be a UTF-8 boundary within the selected output; use next_offset"
                    .into(),
            );
        }
        let mut end = self.offset.saturating_add(self.limit).min(output.len());
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        let complete = end == output.len();
        Ok(
            json!({"fault_id":fault.fault_id,"output_field":field,"sha256":digest,
            "offset":self.offset,"next_offset":if complete { None } else { Some(end) },
            "total_bytes":output.len(),"complete":complete,"text":&output[self.offset..end],
            "evidence_at_unix_micros":observed_at.to_string(),"observation_only":true}),
        )
    }
}
