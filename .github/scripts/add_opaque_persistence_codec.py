from pathlib import Path

path = Path("compiler/src/runtime/persistence.rs")
text = path.read_text()
anchor = "use super::*;\n\n"
if text.count(anchor) != 1:
    raise SystemExit(f"expected top anchor once, found {text.count(anchor)}")

codec = r'''const PERSISTENCE_ENCODING_MAGIC: &[u8; 8] = b"ELANUPST";
const PERSISTENCE_ENCODING_VERSION: u32 = 1;

impl PersistenceImage {
    /// Encode this image into a runtime-owned opaque byte representation.
    ///
    /// The byte format is an implementation compatibility boundary, not Elanu
    /// source syntax or a provider-facing schema.
    pub fn encode(&self) -> Result<Vec<u8>, RuntimeError> {
        let mut encoder = PersistenceEncoder::default();
        encoder.raw(PERSISTENCE_ENCODING_MAGIC);
        encoder.u32(PERSISTENCE_ENCODING_VERSION);
        encode_shape(&mut encoder, &self.shape)?;
        encode_state_values(&mut encoder, &self.state_values)?;
        encode_dynamic_models(&mut encoder, &self.dynamic_models)?;
        encoder.u64(self.next_dynamic_identity);
        Ok(encoder.finish())
    }

    /// Decode one runtime-owned opaque persistence byte payload.
    ///
    /// Encoding validation happens here. Checked-application compatibility is a
    /// separate later check performed by `PersistentRuntime::open`.
    pub fn decode(bytes: &[u8]) -> Result<Self, RuntimeError> {
        let mut decoder = PersistenceDecoder::new(bytes);
        let magic = decoder.raw(PERSISTENCE_ENCODING_MAGIC.len())?;
        if magic != PERSISTENCE_ENCODING_MAGIC {
            return Err(RuntimeError::new("invalid persistence encoding magic"));
        }

        let version = decoder.u32()?;
        if version != PERSISTENCE_ENCODING_VERSION {
            return Err(RuntimeError::new(format!(
                "unsupported persistence encoding version {version}; runtime supports version {PERSISTENCE_ENCODING_VERSION}"
            )));
        }

        let shape = decode_shape(&mut decoder)?;
        let state_values = decode_state_values(&mut decoder)?;
        let dynamic_models = decode_dynamic_models(&mut decoder)?;
        let next_dynamic_identity = decoder.u64()?;
        decoder.finish()?;

        Ok(Self {
            shape,
            state_values,
            dynamic_models,
            next_dynamic_identity,
        })
    }
}

#[derive(Default)]
struct PersistenceEncoder {
    bytes: Vec<u8>,
}

impl PersistenceEncoder {
    fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.raw(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.raw(&value.to_le_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.raw(&value.to_le_bytes());
    }

    fn len(&mut self, value: usize) -> Result<(), RuntimeError> {
        let value = u32::try_from(value)
            .map_err(|_| RuntimeError::new("persistence encoding collection is too large"))?;
        self.u32(value);
        Ok(())
    }

    fn string(&mut self, value: &str) -> Result<(), RuntimeError> {
        self.len(value.len())?;
        self.raw(value.as_bytes());
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

struct PersistenceDecoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> PersistenceDecoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn raw(&mut self, len: usize) -> Result<&'a [u8], RuntimeError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| RuntimeError::new("invalid persistence encoding length overflow"))?;
        if end > self.bytes.len() {
            return Err(RuntimeError::new("truncated persistence encoding"));
        }
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, RuntimeError> {
        Ok(self.raw(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, RuntimeError> {
        let bytes: [u8; 4] = self
            .raw(4)?
            .try_into()
            .expect("decoder requested exactly four bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, RuntimeError> {
        let bytes: [u8; 8] = self
            .raw(8)?
            .try_into()
            .expect("decoder requested exactly eight bytes");
        Ok(u64::from_le_bytes(bytes))
    }

    fn i64(&mut self) -> Result<i64, RuntimeError> {
        let bytes: [u8; 8] = self
            .raw(8)?
            .try_into()
            .expect("decoder requested exactly eight bytes");
        Ok(i64::from_le_bytes(bytes))
    }

    fn len(&mut self) -> Result<usize, RuntimeError> {
        usize::try_from(self.u32()?)
            .map_err(|_| RuntimeError::new("persistence encoding length is unsupported"))
    }

    fn string(&mut self) -> Result<String, RuntimeError> {
        let len = self.len()?;
        let bytes = self.raw(len)?;
        let value = std::str::from_utf8(bytes)
            .map_err(|_| RuntimeError::new("persistence encoding contains invalid UTF-8"))?;
        Ok(value.to_owned())
    }

    fn finish(&self) -> Result<(), RuntimeError> {
        if self.offset != self.bytes.len() {
            return Err(RuntimeError::new(
                "persistence encoding contains trailing bytes",
            ));
        }
        Ok(())
    }
}

fn encode_shape(
    encoder: &mut PersistenceEncoder,
    shape: &PersistenceShape,
) -> Result<(), RuntimeError> {
    encoder.len(shape.static_states.len())?;
    for state in &shape.static_states {
        encoder.string(&state.name)?;
        encode_value_type(encoder, &state.value_type)?;
        encode_designation(encoder, state.designation.as_ref())?;
    }

    encoder.len(shape.model_states.len())?;
    for state in &shape.model_states {
        encoder.string(&state.model_name)?;
        encoder.string(&state.member_name)?;
        encode_value_type(encoder, &state.value_type)?;
        encode_designation(encoder, state.designation.as_ref())?;
    }

    encoder.len(shape.roots.len())?;
    for root in &shape.roots {
        encoder.string(&root.root_name)?;
        encoder.string(&root.model_name)?;
    }
    Ok(())
}

fn decode_shape(decoder: &mut PersistenceDecoder<'_>) -> Result<PersistenceShape, RuntimeError> {
    let static_count = decoder.len()?;
    let mut static_states = Vec::with_capacity(static_count);
    for _ in 0..static_count {
        static_states.push(PersistenceStateShape {
            name: decoder.string()?,
            value_type: decode_value_type(decoder)?,
            designation: decode_designation(decoder)?,
        });
    }

    let model_count = decoder.len()?;
    let mut model_states = Vec::with_capacity(model_count);
    for _ in 0..model_count {
        model_states.push(PersistenceModelStateShape {
            model_name: decoder.string()?,
            member_name: decoder.string()?,
            value_type: decode_value_type(decoder)?,
            designation: decode_designation(decoder)?,
        });
    }

    let root_count = decoder.len()?;
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        roots.push(PersistenceRootShape {
            root_name: decoder.string()?,
            model_name: decoder.string()?,
        });
    }

    Ok(PersistenceShape {
        static_states,
        model_states,
        roots,
    })
}

fn encode_designation(
    encoder: &mut PersistenceEncoder,
    designation: Option<&RuntimeDesignationMetadata>,
) -> Result<(), RuntimeError> {
    match designation {
        None => encoder.u8(0),
        Some(designation) => {
            encoder.u8(1);
            encoder.string(&designation.model_name)?;
            encoder.u8(u8::from(designation.allows_none));
        }
    }
    Ok(())
}

fn decode_designation(
    decoder: &mut PersistenceDecoder<'_>,
) -> Result<Option<RuntimeDesignationMetadata>, RuntimeError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some(RuntimeDesignationMetadata {
            model_name: decoder.string()?,
            allows_none: decode_bool(decoder)?,
        })),
        tag => Err(RuntimeError::new(format!(
            "invalid persistence designation tag {tag}"
        ))),
    }
}

fn encode_value_type(
    encoder: &mut PersistenceEncoder,
    value_type: &ValueType,
) -> Result<(), RuntimeError> {
    match value_type {
        ValueType::Int => encoder.u8(0),
        ValueType::Float => encoder.u8(1),
        ValueType::Bool => encoder.u8(2),
        ValueType::String => encoder.u8(3),
        ValueType::SequenceLive(model) => {
            encoder.u8(4);
            encoder.string(model)?;
        }
        ValueType::Named(name) => {
            encoder.u8(5);
            encoder.string(name)?;
        }
    }
    Ok(())
}

fn decode_value_type(decoder: &mut PersistenceDecoder<'_>) -> Result<ValueType, RuntimeError> {
    match decoder.u8()? {
        0 => Ok(ValueType::Int),
        1 => Ok(ValueType::Float),
        2 => Ok(ValueType::Bool),
        3 => Ok(ValueType::String),
        4 => Ok(ValueType::SequenceLive(decoder.string()?)),
        5 => Ok(ValueType::Named(decoder.string()?)),
        tag => Err(RuntimeError::new(format!(
            "invalid persistence value-type tag {tag}"
        ))),
    }
}

fn encode_value(
    encoder: &mut PersistenceEncoder,
    value: &Value,
) -> Result<(), RuntimeError> {
    match value {
        Value::Int(value) => {
            encoder.u8(0);
            encoder.i64(*value);
        }
        Value::Float(value) => {
            encoder.u8(1);
            encoder.u64(value.to_bits());
        }
        Value::Bool(value) => {
            encoder.u8(2);
            encoder.u8(u8::from(*value));
        }
        Value::String(value) => {
            encoder.u8(3);
            encoder.string(value)?;
        }
        Value::Sequence {
            element_model,
            targets,
        } => {
            encoder.u8(4);
            encoder.string(element_model)?;
            encoder.len(targets.len())?;
            for target in targets {
                encoder.string(target)?;
            }
        }
    }
    Ok(())
}

fn decode_value(decoder: &mut PersistenceDecoder<'_>) -> Result<Value, RuntimeError> {
    match decoder.u8()? {
        0 => Ok(Value::Int(decoder.i64()?)),
        1 => Ok(Value::Float(f64::from_bits(decoder.u64()?))),
        2 => Ok(Value::Bool(decode_bool(decoder)?)),
        3 => Ok(Value::String(decoder.string()?)),
        4 => {
            let element_model = decoder.string()?;
            let count = decoder.len()?;
            let mut targets = Vec::with_capacity(count);
            for _ in 0..count {
                targets.push(decoder.string()?);
            }
            Ok(Value::Sequence {
                element_model,
                targets,
            })
        }
        tag => Err(RuntimeError::new(format!(
            "invalid persistence value tag {tag}"
        ))),
    }
}

fn decode_bool(decoder: &mut PersistenceDecoder<'_>) -> Result<bool, RuntimeError> {
    match decoder.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(RuntimeError::new(format!(
            "invalid persistence boolean value {value}"
        ))),
    }
}

fn encode_state_values(
    encoder: &mut PersistenceEncoder,
    values: &HashMap<String, Value>,
) -> Result<(), RuntimeError> {
    let mut names = values.keys().collect::<Vec<_>>();
    names.sort();
    encoder.len(names.len())?;
    for name in names {
        encoder.string(name)?;
        encode_value(
            encoder,
            values
                .get(name)
                .expect("sorted persistence state name should still exist"),
        )?;
    }
    Ok(())
}

fn decode_state_values(
    decoder: &mut PersistenceDecoder<'_>,
) -> Result<HashMap<String, Value>, RuntimeError> {
    let count = decoder.len()?;
    let mut values = HashMap::with_capacity(count);
    for _ in 0..count {
        let name = decoder.string()?;
        let value = decode_value(decoder)?;
        if values.insert(name.clone(), value).is_some() {
            return Err(RuntimeError::new(format!(
                "persistence encoding repeats state '{name}'"
            )));
        }
    }
    Ok(values)
}

fn encode_dynamic_models(
    encoder: &mut PersistenceEncoder,
    models: &HashMap<String, PersistenceDynamicModel>,
) -> Result<(), RuntimeError> {
    let mut identities = models.keys().collect::<Vec<_>>();
    identities.sort();
    encoder.len(identities.len())?;
    for identity in identities {
        let model = models
            .get(identity)
            .expect("sorted dynamic persistence identity should still exist");
        encoder.string(identity)?;
        encoder.string(&model.model_name)?;
        encoder.string(&model.owner)?;
    }
    Ok(())
}

fn decode_dynamic_models(
    decoder: &mut PersistenceDecoder<'_>,
) -> Result<HashMap<String, PersistenceDynamicModel>, RuntimeError> {
    let count = decoder.len()?;
    let mut models = HashMap::with_capacity(count);
    for _ in 0..count {
        let identity = decoder.string()?;
        let model = PersistenceDynamicModel {
            model_name: decoder.string()?,
            owner: decoder.string()?,
        };
        if models.insert(identity.clone(), model).is_some() {
            return Err(RuntimeError::new(format!(
                "persistence encoding repeats dynamic identity '{identity}'"
            )));
        }
    }
    Ok(models)
}

'''
text = text.replace(anchor, anchor + codec, 1)

# Add byte-only provider and codec tests inside the existing test module.
test_anchor = "    #[derive(Default)]\n    struct MemoryProvider {\n"
if text.count(test_anchor) != 1:
    raise SystemExit(f"expected test provider anchor once, found {text.count(test_anchor)}")

byte_provider = r'''    #[derive(Default)]
    struct ByteMemoryProvider {
        bytes: Option<Vec<u8>>,
        replace_attempts: usize,
    }

    impl PersistenceProvider for ByteMemoryProvider {
        fn load(&mut self) -> Result<Option<PersistenceImage>, RuntimeError> {
            self.bytes
                .as_deref()
                .map(PersistenceImage::decode)
                .transpose()
        }

        fn replace(&mut self, image: &PersistenceImage) -> Result<(), RuntimeError> {
            self.replace_attempts += 1;
            self.bytes = Some(image.encode()?);
            Ok(())
        }
    }

'''
text = text.replace(test_anchor, byte_provider + test_anchor, 1)

end_anchor = "\n    #[test]\n    fn action_body_change_with_same_persisted_shape_remains_compatible() {\n"
if text.count(end_anchor) != 1:
    raise SystemExit(f"expected final-test anchor once, found {text.count(end_anchor)}")

codec_tests = r'''
    #[test]
    fn opaque_bytes_round_trip_across_provider_recreation() {
        let mut first = PersistentRuntime::open(checked(), ByteMemoryProvider::default()).unwrap();
        first.run_action("createFolder").unwrap();
        let provider = first.into_provider();
        let durable_bytes = provider.bytes.expect("accepted action should produce bytes");

        // Process-equivalent restart: only the byte payload survives. Neither the
        // prior runtime nor a PersistenceImage object is carried across.
        let provider = ByteMemoryProvider {
            bytes: Some(durable_bytes),
            replace_attempts: 0,
        };
        let mut restarted = PersistentRuntime::open(checked(), provider)
            .expect("runtime-owned bytes should decode and restore");

        assert_eq!(
            restarted.value("selectedName").unwrap(),
            Value::String("Project".to_string())
        );
        restarted.run_action("rename").unwrap();
        assert_eq!(
            restarted.value("selectedName").unwrap(),
            Value::String("Renamed".to_string())
        );
    }

    #[test]
    fn encoding_is_deterministic_for_the_same_committed_image() {
        let mut app = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        app.run_action("createFolder").unwrap();
        let image = app.provider.image.clone().unwrap();

        assert_eq!(image.encode().unwrap(), image.encode().unwrap());
        assert_eq!(PersistenceImage::decode(&image.encode().unwrap()).unwrap(), image);
    }

    #[test]
    fn malformed_bytes_fail_during_decode_before_runtime_restore() {
        let error = PersistenceImage::decode(b"not-an-elanu-persistence-image")
            .expect_err("bad magic must fail during decoding");
        assert!(error.message.contains("magic"));

        let truncated = PERSISTENCE_ENCODING_MAGIC.to_vec();
        let error = PersistenceImage::decode(&truncated)
            .expect_err("missing version/body must fail during decoding");
        assert!(error.message.contains("truncated"));
    }

    #[test]
    fn encoding_version_failure_is_distinct_from_application_shape_mismatch() {
        let mut app = PersistentRuntime::open(checked(), MemoryProvider::default()).unwrap();
        app.run_action("createFolder").unwrap();
        let image = app.provider.image.clone().unwrap();
        let mut bytes = image.encode().unwrap();
        let version_offset = PERSISTENCE_ENCODING_MAGIC.len();
        bytes[version_offset..version_offset + 4].copy_from_slice(&999_u32.to_le_bytes());

        let version_error = PersistenceImage::decode(&bytes)
            .expect_err("unsupported encoding version must fail during decode");
        assert!(version_error.message.contains("encoding version"));

        let incompatible_source = SOURCE.replace(
            "state name = \"\"",
            "state name = \"\"\n    state archived = false",
        );
        let incompatible = check_source_with_runtime_models(&incompatible_source).unwrap();
        let provider = ByteMemoryProvider {
            bytes: Some(image.encode().unwrap()),
            replace_attempts: 0,
        };
        let shape_error = PersistentRuntime::open(incompatible, provider)
            .err()
            .expect("decoded image with incompatible application shape must fail");
        assert!(shape_error.message.contains("checked application shape"));
        assert!(!shape_error.message.contains("encoding version"));
    }

    #[test]
    fn action_body_change_remains_compatible_after_byte_decode() {
        let mut first = PersistentRuntime::open(checked(), ByteMemoryProvider::default()).unwrap();
        first.run_action("createFolder").unwrap();
        let provider = first.into_provider();

        let changed_source = SOURCE.replace("\"Renamed\"", "\"Renamed after byte restart\"");
        let changed = check_source_with_runtime_models(&changed_source).unwrap();
        let mut restarted = PersistentRuntime::open(changed, provider)
            .expect("action-only change should remain compatible after decode");
        restarted.run_action("rename").unwrap();
        assert_eq!(
            restarted.value("selectedName").unwrap(),
            Value::String("Renamed after byte restart".to_string())
        );
    }

'''
text = text.replace(end_anchor, "\n" + codec_tests + end_anchor, 1)
path.write_text(text)

# Document the host-level codec without changing source-language docs.
dev = Path("docs/DEVELOPMENT.md")
dev_text = dev.read_text()
needle = "This boundary does not select a disk/database format, source `save`/`load`, schema migration,\nasync durability, crash recovery, retries, or general external-effect semantics.\n"
replacement = needle + "\n`PersistenceImage::encode` and `PersistenceImage::decode` provide the runtime-owned opaque byte\nboundary for process-to-process storage. The runtime format carries its own encoding version and\nvalidates malformed or unsupported payloads before application-shape compatibility and restore.\nProviders may store these bytes but must not interpret the private payload. The byte format remains\nruntime implementation compatibility rather than Elanu source semantics or a provider schema.\n"
if dev_text.count(needle) != 1:
    raise SystemExit(f"expected DEVELOPMENT codec anchor once, found {dev_text.count(needle)}")
dev.write_text(dev_text.replace(needle, replacement, 1))

changelog = Path("CHANGELOG.md")
changelog_text = changelog.read_text()
needle = "### Compiler/runtime\n\n"
entry = "- Added runtime-owned deterministic opaque persistence image encoding/decoding with explicit format versioning and validation, allowing providers to store uninterpreted bytes across process restart without exposing image internals or selecting a storage backend.\n"
if changelog_text.count(needle) < 1:
    raise SystemExit("CHANGELOG compiler/runtime anchor missing")
changelog.write_text(changelog_text.replace(needle, needle + entry, 1))
