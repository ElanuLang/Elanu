pub const RUNTIME_SEQUENCE_TYPE_PREFIX: &str = "__elanu_runtime_sequence_live$";
const RUNTIME_SEQUENCE_VALUE_PREFIX: &str = "__elanu_runtime_sequence_value$";

pub fn encode_runtime_sequence_type(model: &str) -> String {
    format!("{RUNTIME_SEQUENCE_TYPE_PREFIX}{model}")
}

pub fn decode_runtime_sequence_type(name: &str) -> Option<&str> {
    name.strip_prefix(RUNTIME_SEQUENCE_TYPE_PREFIX)
}

pub fn encode_runtime_sequence_value(model: &str, targets: &[String]) -> String {
    let targets = targets
        .iter()
        .map(|target| hex_encode(target))
        .collect::<Vec<_>>()
        .join("$");
    format!(
        "{RUNTIME_SEQUENCE_VALUE_PREFIX}{}${targets}",
        hex_encode(model)
    )
}

pub fn decode_runtime_sequence_value(value: &str) -> Option<(String, Vec<String>)> {
    let rest = value.strip_prefix(RUNTIME_SEQUENCE_VALUE_PREFIX)?;
    let (model, targets) = rest.split_once('$')?;
    let model = hex_decode(model)?;
    let targets = if targets.is_empty() {
        Vec::new()
    } else {
        targets
            .split('$')
            .map(hex_decode)
            .collect::<Option<Vec<_>>>()?
    };
    Some((model, targets))
}

fn hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_decode(value: &str) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}
