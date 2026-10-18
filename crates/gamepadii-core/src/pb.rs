//! A small protobuf codec: the wire format only, no schema. It reads and
//! writes the fields of one message, in their order, and keeps the fields
//! it doesn't know as raw bytes. A protobuf crate can replace it later.

/// The value of one field, by wire type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Wire type 0: `bool`, enums, `int32`, `uint32`. A negative `int32`
    /// is 10 bytes, sign-extended to 64 bits.
    Varint(u64),
    /// Wire type 2: `bytes`, strings, messages.
    Bytes(Vec<u8>),
    /// Wire type 5.
    Fixed32(u32),
    /// Wire type 1.
    Fixed64(u64),
}

/// One field: its number and value.
pub type Field = (u32, Value);

impl Value {
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            // int32 truncates on the wire: the low 32 bits.
            Value::Varint(v) => Some(*v as u32 as i32),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Value::Varint(v) => u32::try_from(*v).ok(),
            _ => None,
        }
    }

    /// An `int32` value, as protobuf encodes it.
    pub fn int32(v: i32) -> Self {
        Value::Varint(i64::from(v) as u64)
    }
}

fn varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*at)?;
        *at += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn take<const N: usize>(b: &[u8], at: &mut usize) -> Option<[u8; N]> {
    let bytes = b.get(*at..*at + N)?.try_into().ok()?;
    *at += N;
    Some(bytes)
}

/// The fields of a message, in order. `None` for bytes that are not
/// protobuf: a bad wire type, field number 0, or a short field.
pub fn decode(b: &[u8]) -> Option<Vec<Field>> {
    let mut at = 0;
    let mut fields = Vec::new();
    while at < b.len() {
        let key = varint(b, &mut at)?;
        let number = u32::try_from(key >> 3).ok().filter(|n| *n != 0)?;
        let value = match key & 7 {
            0 => Value::Varint(varint(b, &mut at)?),
            1 => Value::Fixed64(u64::from_le_bytes(take(b, &mut at)?)),
            2 => {
                let len = usize::try_from(varint(b, &mut at)?).ok()?;
                let v = b.get(at..at.checked_add(len)?)?.to_vec();
                at += len;
                Value::Bytes(v)
            }
            5 => Value::Fixed32(u32::from_le_bytes(take(b, &mut at)?)),
            _ => return None,
        };
        fields.push((number, value));
    }
    Some(fields)
}

/// One field, encoded.
pub fn encode_field(out: &mut Vec<u8>, number: u32, value: &Value) {
    let wire = match value {
        Value::Varint(_) => 0,
        Value::Fixed64(_) => 1,
        Value::Bytes(_) => 2,
        Value::Fixed32(_) => 5,
    };
    put_varint(out, u64::from(number) << 3 | wire);
    match value {
        Value::Varint(v) => put_varint(out, *v),
        Value::Fixed64(v) => out.extend_from_slice(&v.to_le_bytes()),
        Value::Bytes(v) => {
            put_varint(out, v.len() as u64);
            out.extend_from_slice(v);
        }
        Value::Fixed32(v) => out.extend_from_slice(&v.to_le_bytes()),
    }
}

pub fn encode(fields: &[Field]) -> Vec<u8> {
    let mut out = Vec::new();
    for (number, value) in fields {
        encode_field(&mut out, *number, value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip() {
        let fields = vec![
            (1, Value::Varint(1)),
            (4, Value::int32(-1)),
            (5, Value::Bytes(vec![2; 30])),
            (21, Value::Varint(300)),
            (22, Value::Fixed32(7)),
            (23, Value::Fixed64(8)),
        ];
        let b = encode(&fields);
        // Field 4, -1: a 10-byte varint.
        assert_eq!(
            &b[2..13],
            &[
                0x20, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01
            ]
        );
        // Field 21 has a 2-byte key.
        assert_eq!(&b[45..49], &[0xa8, 0x01, 0xac, 0x02]);
        let back = decode(&b).unwrap();
        assert_eq!(back, fields);
        assert_eq!(back[1].1.as_i32(), Some(-1));
        assert_eq!(back[3].1.as_u32(), Some(300));
    }

    #[test]
    fn bad_bytes_are_not_protobuf() {
        assert_eq!(decode(&[]), Some(vec![]));
        assert_eq!(decode(&[0x08]), None, "no value");
        assert_eq!(decode(&[0x0b, 0x00]), None, "wire type 3");
        assert_eq!(decode(&[0x00, 0x00]), None, "field 0");
        assert_eq!(decode(&[0x2a, 0x05, 1, 2]), None, "short bytes");
        assert_eq!(decode(&[0xff; 100]), None);
    }
}
