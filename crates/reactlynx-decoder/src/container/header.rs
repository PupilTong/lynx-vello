//! Top-level header decoding.

use crate::{
    container::header_ext_info,
    error::{DecodeError, Result},
    model::{CompileOptions, Header},
    reader::Reader,
    value::{Value, decode_value},
    version::{V_1_6, V_2_7, Version},
};

const QUICK_BINARY_MAGIC: u32 = 0x0024_1922;
const LEPUS_BINARY_MAGIC: u32 = 0xdd73_7199;
const MIN_SUPPORTED_VERSION: &str = "0.1.0.0";

pub(super) struct DecodedHeader<'a> {
    pub(super) header: Header<'a>,
    pub(super) compile_options: CompileOptions<'a>,
    pub(super) template_info: Option<Value<'a>>,
}

pub(super) fn decode<'a>(reader: &mut Reader<'a>) -> Result<DecodedHeader<'a>> {
    // C++: DecodeHeader validates total_size first
    // (lynx_binary_base_template_reader_impl.cc:58-72).
    let total_size = reader.u32()?;
    if usize::try_from(total_size).map_or(true, |declared| declared != reader.len()) {
        return Err(DecodeError::SizeMismatch {
            declared: total_size,
            actual: reader.len(),
        });
    }

    // C++: DecodeMagicWord dispatches Quick/Lepus VM magic
    // (lynx_binary_base_template_reader.cc:22-42).
    let magic = reader.u32()?;
    match magic {
        QUICK_BINARY_MAGIC => {}
        LEPUS_BINARY_MAGIC => return Err(DecodeError::UnsupportedVm),
        other => return Err(DecodeError::BadMagic(other)),
    }

    let lepus_version = reader.lstr()?;
    let (cli_version, ios_version, android_version, target_sdk_str) =
        if lepus_version > MIN_SUPPORTED_VERSION {
            let cli = reader.lstr()?;
            let ios = reader.lstr()?;
            let android = reader.lstr()?;
            (Some(cli), Some(ios), Some(android), ios)
        } else {
            (None, None, None, "")
        };
    let target_sdk = Version::parse(target_sdk_str);
    let mut compile_options = CompileOptions::defaults(target_sdk_str);

    // C++: header_ext_info is gated by FEATURE_HEADER_EXT_INFO_VERSION
    // (lynx_binary_base_template_reader_impl.cc:122-127).
    if target_sdk.is_at_least(V_1_6) {
        header_ext_info::decode(reader, &mut compile_options)?;
    }

    // C++: template_info is gated by FEATURE_TEMPLATE_INFO
    // (lynx_binary_base_template_reader_impl.cc:130-133).
    let template_info = if compile_options.target_sdk.is_at_least(V_2_7) {
        Some(decode_value(reader)?)
    } else {
        None
    };

    // C++: trial_options is decoded for compatibility then discarded
    // (lynx_binary_base_template_reader_impl.cc:136-141).
    if compile_options.enable_trial_options {
        let _discarded = decode_value(reader)?;
    }

    Ok(DecodedHeader {
        header: Header {
            total_size,
            magic,
            lepus_version,
            cli_version,
            ios_version,
            android_version,
            target_sdk,
        },
        compile_options,
        template_info,
    })
}
