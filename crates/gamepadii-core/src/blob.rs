//! The saved config as one blob (control channel v1.6, §3.14 of
//! `docs/design/control-channel.md`): what a backup keeps of it. The blob
//! is a `Config` message, protobuf encoded, as the firmware saves it.
//! gamepadii does not decode it: it keeps the bytes, and checks their size,
//! their CRC-32 and their protobuf structure.

use serde::{Deserialize, Serialize};

use crate::channel::{Channel, proto};
use crate::error::{Error, Result};
use crate::pb;

/// The saved config in a backup file (`config_blob`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBlob {
    /// The bytes of the blob.
    pub size: u32,
    /// The CRC-32 of the blob (zlib's), as the pad gave it.
    pub crc32: u32,
    /// The firmware version of the pad that gave it (STRING 1).
    pub firmware: Option<String>,
    /// The board config of that pad (STRING 2): `Haute42COSMOX`.
    pub board: Option<String>,
    /// The board ID of that pad (protocol 1.3); `null` before 1.3.
    pub board_id: Option<String>,
    /// The blob, in lower-case hex.
    pub hex: String,
}

impl ConfigBlob {
    /// READ_CONFIG of the whole image, and the strings that go with it.
    pub fn read(ch: &Channel) -> Result<Self> {
        let image = ch.read_config_image()?;
        Ok(Self {
            size: image.head.size,
            crc32: image.head.crc,
            firmware: ch.string(proto::string::VERSION).ok().flatten(),
            board: ch.string(proto::string::BOARD).ok().flatten(),
            board_id: ch.board_id().ok().flatten(),
            hex: hex(&image.data),
        })
    }

    /// The bytes, when they agree with `size` and `crc32` and are protobuf;
    /// else a usage error that says what is wrong.
    pub fn data(&self) -> Result<Vec<u8>> {
        let data = unhex(&self.hex).ok_or_else(|| {
            Error::Usage("the config blob is not hex: the file is damaged".into())
        })?;
        if data.len() != self.size as usize {
            return Err(Error::Usage(format!(
                "the config blob has {} bytes, but its size is {}: the file is damaged",
                data.len(),
                self.size
            )));
        }
        let crc = proto::crc32(&data);
        if crc != self.crc32 {
            return Err(Error::Usage(format!(
                "the CRC-32 of the config blob is {crc:08x}, not {:08x}: the file is damaged",
                self.crc32
            )));
        }
        if data.is_empty() {
            return Err(Error::Usage(
                "the config blob is empty: the pad had no saved config".into(),
            ));
        }
        if pb::decode(&data).is_none() {
            return Err(Error::Usage(
                "the config blob is not protobuf: the file is damaged".into(),
            ));
        }
        Ok(data)
    }

    pub fn info(&self) -> BlobInfo {
        BlobInfo {
            size: self.size,
            crc32: self.crc32,
            firmware: self.firmware.clone(),
        }
    }
}

/// What a backup report says about the blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlobInfo {
    pub size: u32,
    pub crc32: u32,
    pub firmware: Option<String>,
}

/// `data` in lower-case hex.
pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes of a hex string (either case); `None` when it is not hex.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(data: &[u8]) -> ConfigBlob {
        ConfigBlob {
            size: data.len() as u32,
            crc32: proto::crc32(data),
            firmware: None,
            board: None,
            board_id: None,
            hex: hex(data),
        }
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
        assert_eq!(unhex("00AB10"), Some(vec![0, 0xab, 0x10]));
        assert_eq!(unhex(""), Some(vec![]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("+1"), None);
    }

    #[test]
    fn a_damaged_blob_is_refused() {
        let ok = [0x08, 0x01, 0x12, 0x02, 0x08, 0x05];
        assert_eq!(blob(&ok).data().unwrap(), ok);
        let e = |b: ConfigBlob| b.data().unwrap_err().to_string();
        assert!(
            e(ConfigBlob {
                size: 5,
                ..blob(&ok)
            })
            .contains("6 bytes, but its size is 5")
        );
        assert!(
            e(ConfigBlob {
                crc32: 1,
                ..blob(&ok)
            })
            .contains("not 00000001")
        );
        assert!(
            e(ConfigBlob {
                hex: "x".into(),
                ..blob(&ok)
            })
            .contains("not hex")
        );
        assert!(e(blob(&[0x0b, 0x00])).contains("not protobuf"));
        assert!(e(blob(&[])).contains("empty"));
        let damaged = ConfigBlob {
            size: 5,
            ..blob(&ok)
        }
        .data()
        .unwrap_err();
        assert_eq!(damaged.exit_code(), crate::error::exit::USAGE);
    }
}
