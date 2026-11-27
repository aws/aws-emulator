// SPDX-License-Identifier: CC0-1.0

//! The PIN computations behind this service's PIN data operations.
//!
//! A PIN is never carried in the clear by this API: it travels inside an ISO 9564 PIN
//! BLOCK encrypted under a PIN encryption key, and what the generating and verifying
//! operations exchange about it is a derived value — a Visa PIN verification value or
//! an IBM 3624 PIN offset — computed under a separate PIN verification key. Three
//! published algorithms, each written out here over the block ciphers of the parent
//! module because no general crate implements a payment-industry PIN algorithm:
//!
//! 1. **the ISO 9564 PIN block formats** the modelled `PinBlockFormatForPinData`
//!    names — format 0 (the ANSI X9.8 block, PIN field XORed with the account number
//!    field), format 1 (PIN field with random fill and no account number), format 3
//!    (format 0 with the fill drawn from the values 10 to 15) and format 4 (the AES
//!    block, which binds the account number by encrypting, XORing and encrypting
//!    again). Each one is a function from a PIN to a block and back, so a PIN encrypted
//!    into a block under a key is the PIN read back out of that block under the same
//!    key.
//! 2. **the IBM 3624 PIN and offset**: the cardholder's validation data padded to one
//!    cipher block, encrypted under the PIN verification key and decimalised through
//!    the request's own table gives the NATURAL PIN, and the offset is the digitwise
//!    difference, modulo ten, between the PIN the cardholder holds and that natural
//!    PIN — which is what makes the offset storable in the clear and the PIN
//!    recoverable from it.
//! 3. **the Visa PIN verification value**: the account number, the PIN verification
//!    key index and the PIN form a sixteen-digit transformed security parameter, which
//!    is encrypted under the PIN verification key and decimalised to four digits.
//!
//! Every value here is a function of the key's working material and the request alone,
//! so each one is reproducible: that is what lets `VerifyPinData` recompute what
//! `GeneratePinData` answered and compare the two. No PIN and no PIN block is stored
//! anywhere, and no refusal worded here quotes a PIN, a PIN block or a derived value
//! back.

use ::rand::Rng;

use super::{decimalised, BlockCipher};

/// Why a PIN computation could not be performed.
#[derive(Debug, Clone)]
pub enum PinRefusal {
    /// The request cannot be served as asked — a key whose algorithm names no cipher
    /// the format runs under, an account number the block cannot carry, a length no
    /// PIN has. The modelled `ValidationException`.
    Invalid(String),
    /// The PIN block decrypted to no PIN field of the format the request named, so the
    /// PIN inside it cannot be read at all. A verifying operation answers this as a
    /// failed verification; a generating one refuses the request.
    Unreadable(String),
}

/// A PIN computation's result.
type Pin<T> = Result<T, PinRefusal>;

/// The modelled `PinBlockFormatForPinData` values, each the ISO 9564 PIN block format
/// of the same number (`smithy_query(query="shape",shape="PinBlockFormatForPinData")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinBlockFormat {
    /// ISO format 0, "equivalent to the ANSI X9.8, VISA-1, and ECI-1 PIN block
    /// formats" (that shape's own documentation).
    Iso0,
    /// ISO format 1, whose fill digits are drawn and which binds no account number.
    Iso1,
    /// ISO format 3, "the same as ISO_Format_0 except that the fill digits are random
    /// values from 10 to 15".
    Iso3,
    /// ISO format 4, "the only one supporting AES encryption".
    Iso4,
}

/// How many nibbles the Triple DES formats' fields carry: that cipher's eight-byte
/// block.
const TDES_BLOCK_NIBBLES: usize = 16;

/// How many nibbles the ISO format 4 fields carry: the AES sixteen-byte block.
const AES_BLOCK_NIBBLES: usize = 32;

/// How many bytes the Triple DES block is.
const TDES_BLOCK_BYTES: usize = TDES_BLOCK_NIBBLES / 2;

/// How many bytes the AES block is.
const AES_BLOCK_BYTES: usize = AES_BLOCK_NIBBLES / 2;

/// The fill nibble of an ISO format 0 PIN field.
const ISO_0_FILL: u8 = 0x0F;

/// The lowest fill nibble of an ISO format 3 PIN field: "the fill digits are random
/// values from 10 to 15" (`PinBlockFormatForPinData`).
const ISO_3_FILL_LOW: u8 = 10;

/// The pad nibble of an ISO format 4 PIN field.
const ISO_4_PAD: u8 = 0x0A;

/// The shortest account number the model admits (`PrimaryAccountNumberType`, length
/// 12..19), which is the length an ISO format 4 account-number field counts from.
const SHORTEST_ACCOUNT_NUMBER: usize = 12;

/// How many account-number digits the ISO format 0 and 3 account-number field carries:
/// the twelve rightmost digits of the account number excluding its check digit.
const PAN_FIELD_DIGITS: usize = 12;

/// The shortest PIN the model admits (`IntegerRangeBetween4And12`).
const SHORTEST_PIN: usize = 4;

/// The longest PIN the model admits (`IntegerRangeBetween4And12`).
const LONGEST_PIN: usize = 12;

impl PinBlockFormat {
    /// The format `name` names, or the reason it names none.
    ///
    /// The names are matched exactly against the modelled enum, so a format the model
    /// gains later is refused rather than served as a neighbouring one.
    pub fn parse(name: &str) -> Pin<Self> {
        match name {
            "ISO_FORMAT_0" => Ok(Self::Iso0),
            "ISO_FORMAT_1" => Ok(Self::Iso1),
            "ISO_FORMAT_3" => Ok(Self::Iso3),
            "ISO_FORMAT_4" => Ok(Self::Iso4),
            other => Err(PinRefusal::Invalid(format!(
                "PinBlockFormat {other} is not a valid PIN block format."
            ))),
        }
    }

    /// The nibble an ISO 9564 PIN field of this format opens with, which is the format's
    /// own number.
    fn control_nibble(self) -> u8 {
        match self {
            Self::Iso0 => 0,
            Self::Iso1 => 1,
            Self::Iso3 => 3,
            Self::Iso4 => 4,
        }
    }

    /// Whether a block of this format binds the account number — every format but
    /// format 1, whose PIN field stands alone.
    #[must_use]
    pub fn binds_account_number(self) -> bool {
        self != Self::Iso1
    }

    /// How many bytes one block of this format is.
    fn block_len(self) -> usize {
        match self {
            Self::Iso4 => AES_BLOCK_BYTES,
            _ => TDES_BLOCK_BYTES,
        }
    }

    /// The cipher a PIN encryption key of `key_algorithm` keys for this format, or the
    /// reason that key cannot carry it.
    ///
    /// "The ISO_Format_4 PIN block format is the only one supporting AES encryption"
    /// (`PinBlockFormatForPinData`): format 4 is the AES block and the other three are
    /// the Triple DES block, so the key's algorithm has to name the cipher whose block
    /// the format is.
    fn cipher(self, key_algorithm: &str, material: &[u8]) -> Pin<BlockCipher> {
        let cipher = BlockCipher::new(key_algorithm, material).filter(|cipher| {
            cipher.block_len() == self.block_len()
        });
        cipher.ok_or_else(|| {
            PinRefusal::Invalid(format!(
                "KeyAlgorithm {key_algorithm} is not valid for PinBlockFormat {}.",
                self.name()
            ))
        })
    }

    /// This format's modelled name, for a refusal to state.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Iso0 => "ISO_FORMAT_0",
            Self::Iso1 => "ISO_FORMAT_1",
            Self::Iso3 => "ISO_FORMAT_3",
            Self::Iso4 => "ISO_FORMAT_4",
        }
    }

    /// The PIN field of `pin` in this format: the control nibble, the PIN's length, its
    /// digits, and this format's own fill.
    fn pin_field(self, pin: &str) -> Pin<Vec<u8>> {
        let digits = decimal_nibbles("PIN", pin)?;
        if !(SHORTEST_PIN..=LONGEST_PIN).contains(&digits.len()) {
            return Err(PinRefusal::Invalid(format!(
                "A PIN of {} digits is outside the {SHORTEST_PIN} to {LONGEST_PIN} digits a PIN \
                 block carries.",
                digits.len()
            )));
        }
        let width = self.block_len() * 2;
        let mut field = Vec::with_capacity(width);
        field.push(self.control_nibble());
        // The length nibble is the PIN's own digit count, which is how the PIN is read
        // back out of a block whatever the fill around it.
        field.push(u8::try_from(digits.len()).unwrap_or_default());
        field.extend_from_slice(&digits);
        let mut generator = ::rand::rng();
        while field.len() < width {
            // The pad and fill of each format: format 0's fixed `F`, format 1's drawn
            // nibble, format 3's nibble drawn from 10 to 15, and format 4's `A` pad
            // followed by a drawn second half.
            let nibble = match self {
                Self::Iso0 => ISO_0_FILL,
                Self::Iso1 => generator.random_range(0..=0x0F),
                Self::Iso3 => generator.random_range(ISO_3_FILL_LOW..=0x0F),
                Self::Iso4 if field.len() < TDES_BLOCK_NIBBLES => ISO_4_PAD,
                Self::Iso4 => generator.random_range(0..=0x0F),
            };
            field.push(nibble);
        }
        Ok(field)
    }

    /// The PIN inside `field`, or the reason `field` is no PIN field of this format.
    fn pin_of(self, field: &[u8]) -> Pin<String> {
        let unreadable = || {
            PinRefusal::Unreadable(format!(
                "The PIN block does not carry a PIN in PinBlockFormat {}.",
                self.name()
            ))
        };
        let (Some(control), Some(length)) = (field.first(), field.get(1)) else {
            return Err(unreadable());
        };
        if *control != self.control_nibble() {
            return Err(unreadable());
        }
        let length = usize::from(*length);
        if !(SHORTEST_PIN..=LONGEST_PIN).contains(&length) {
            return Err(unreadable());
        }
        let digits = field.get(2..2 + length).ok_or_else(unreadable)?;
        let mut pin = String::with_capacity(length);
        for nibble in digits {
            let digit = char::from_digit(u32::from(*nibble), 10).ok_or_else(unreadable)?;
            pin.push(digit);
        }
        Ok(pin)
    }

    /// The account-number field of a block of this format, which `pin_field` is
    /// combined with.
    ///
    /// Format 0 and format 3 take the twelve rightmost digits of the account number
    /// excluding its check digit, after four zero nibbles. Format 4 takes the account
    /// number's own length, counted from the shortest the model admits, then every
    /// digit of it, and zero nibbles for the rest.
    fn account_number_field(self, pan: &str) -> Pin<Vec<u8>> {
        let digits = decimal_nibbles("PrimaryAccountNumber", pan)?;
        let width = self.block_len() * 2;
        let mut field = vec![0u8; width];
        match self {
            Self::Iso4 => {
                let Some(excess) = digits.len().checked_sub(SHORTEST_ACCOUNT_NUMBER) else {
                    return Err(PinRefusal::Invalid(format!(
                        "An account number of {} digits is shorter than the \
                         {SHORTEST_ACCOUNT_NUMBER} digits a PIN block carries.",
                        digits.len()
                    )));
                };
                field[0] = u8::try_from(excess).unwrap_or_default();
                for (slot, digit) in field[1..].iter_mut().zip(digits.iter()) {
                    *slot = *digit;
                }
            }
            _ => {
                // The check digit is the account number's last, and the field carries
                // the twelve digits before it — left-padded with zeros for an account
                // number that has fewer.
                let Some(without_check_digit) = digits.split_last().map(|(_, rest)| rest) else {
                    return Err(PinRefusal::Invalid(
                        "An empty account number carries no PIN block digits.".to_string(),
                    ));
                };
                let from = without_check_digit
                    .len()
                    .saturating_sub(PAN_FIELD_DIGITS);
                let tail = &without_check_digit[from..];
                let start = width - tail.len();
                field[start..].copy_from_slice(tail);
            }
        }
        Ok(field)
    }
}

/// The PIN block of `pin`, encrypted under the PIN encryption key `key_algorithm` and
/// `material` name.
///
/// `pan` is the account number every format but format 1 binds; a format that binds one
/// and is handed none is refused rather than served over a block the account number is
/// missing from.
pub fn encrypt_pin_block(
    format: PinBlockFormat,
    key_algorithm: &str,
    material: &[u8],
    pin: &str,
    pan: Option<&str>,
) -> Pin<String> {
    let cipher = format.cipher(key_algorithm, material)?;
    let field = format.pin_field(pin)?;
    let block = match format {
        PinBlockFormat::Iso4 => {
            let account = format.account_number_field(account_number(format, pan)?)?;
            let mut state = bytes_of(&field);
            cipher.encrypt_block(&mut state);
            xor(&mut state, &bytes_of(&account));
            cipher.encrypt_block(&mut state);
            state
        }
        PinBlockFormat::Iso1 => {
            let mut state = bytes_of(&field);
            cipher.encrypt_block(&mut state);
            state
        }
        _ => {
            let account = format.account_number_field(account_number(format, pan)?)?;
            let mut state = bytes_of(&field);
            xor(&mut state, &bytes_of(&account));
            cipher.encrypt_block(&mut state);
            state
        }
    };
    Ok(hex::encode_upper(block))
}

/// The PIN inside `encrypted`, read under the PIN encryption key `key_algorithm` and
/// `material` name — the inverse of [`encrypt_pin_block`].
pub fn decrypt_pin_block(
    format: PinBlockFormat,
    key_algorithm: &str,
    material: &[u8],
    encrypted: &str,
    pan: Option<&str>,
) -> Pin<String> {
    let cipher = format.cipher(key_algorithm, material)?;
    let mut state = hex::decode(encrypted).map_err(|_| {
        PinRefusal::Invalid("The encrypted PIN block is not hexadecimal.".to_string())
    })?;
    if state.len() != format.block_len() {
        return Err(PinRefusal::Invalid(format!(
            "An encrypted PIN block in PinBlockFormat {} carries {} hexadecimal characters, not {}.",
            format.name(),
            format.block_len() * 2,
            encrypted.len()
        )));
    }
    let field = match format {
        PinBlockFormat::Iso4 => {
            let account = format.account_number_field(account_number(format, pan)?)?;
            cipher.decrypt_block(&mut state);
            xor(&mut state, &bytes_of(&account));
            cipher.decrypt_block(&mut state);
            state
        }
        PinBlockFormat::Iso1 => {
            cipher.decrypt_block(&mut state);
            state
        }
        _ => {
            let account = format.account_number_field(account_number(format, pan)?)?;
            cipher.decrypt_block(&mut state);
            xor(&mut state, &bytes_of(&account));
            state
        }
    };
    format.pin_of(&nibbles_of(&field))
}

/// The account number a format that binds one was handed, or the refusal of a request
/// that named none.
fn account_number(format: PinBlockFormat, pan: Option<&str>) -> Pin<&str> {
    pan.ok_or_else(|| {
        PinRefusal::Invalid(format!(
            "PrimaryAccountNumber is required for PinBlockFormat {}.",
            format.name()
        ))
    })
}

/// How many digits a Visa PIN verification value carries.
const PVV_DIGITS: usize = 4;

/// How many PIN digits the transformed security parameter carries: the PIN's leading
/// four.
const TSP_PIN_DIGITS: usize = 4;

/// How many account-number digits it carries: the eleven rightmost digits of the
/// account number excluding its check digit.
const TSP_PAN_DIGITS: usize = 11;

/// The Visa PIN verification value of `pin` for `pan` under the PIN verification key
/// `key_algorithm` and `material` name.
///
/// The published computation: the eleven rightmost digits of the account number
/// excluding its check digit, the PIN verification key index and the PIN's leading four
/// digits form a sixteen-digit transformed security parameter; that parameter is
/// encrypted under the key and the result's hexadecimal characters are decimalised —
/// the decimal characters in order first, then the remaining characters each reduced by
/// ten — and the leading four of them are the value.
pub fn visa_pin_verification_value(
    key_algorithm: &str,
    material: &[u8],
    pan: &str,
    pin_verification_key_index: i32,
    pin: &str,
) -> Pin<String> {
    let pan_digits = decimal_nibbles("PrimaryAccountNumber", pan)?;
    let Some(without_check_digit) = pan_digits.split_last().map(|(_, rest)| rest) else {
        return Err(PinRefusal::Invalid(
            "An empty account number carries no transformed security parameter.".to_string(),
        ));
    };
    if without_check_digit.len() < TSP_PAN_DIGITS {
        return Err(PinRefusal::Invalid(format!(
            "An account number of {} digits is shorter than the {} digits a transformed security \
             parameter carries.",
            pan_digits.len(),
            TSP_PAN_DIGITS + 1
        )));
    }
    let index = u8::try_from(pin_verification_key_index)
        .ok()
        .filter(|index| *index < 10)
        .ok_or_else(|| {
            PinRefusal::Invalid(format!(
                "PinVerificationKeyIndex {pin_verification_key_index} is not a single digit."
            ))
        })?;
    let pin_digits = decimal_nibbles("PIN", pin)?;
    if pin_digits.len() < TSP_PIN_DIGITS {
        return Err(PinRefusal::Invalid(format!(
            "A PIN of {} digits is shorter than the {TSP_PIN_DIGITS} digits a transformed \
             security parameter carries.",
            pin_digits.len()
        )));
    }
    let mut parameter: Vec<u8> =
        without_check_digit[without_check_digit.len() - TSP_PAN_DIGITS..].to_vec();
    parameter.push(index);
    parameter.extend_from_slice(&pin_digits[..TSP_PIN_DIGITS]);
    let mut state = bytes_of(&parameter);
    let cipher = verification_cipher(key_algorithm, material)?;
    cipher.encrypt_block(&mut state);
    let value = decimalised(&hex::encode_upper(state), PVV_DIGITS);
    if value.len() < PVV_DIGITS {
        return Err(PinRefusal::Invalid(
            "The PIN verification value could not be computed.".to_string(),
        ));
    }
    Ok(value)
}

/// How many characters the IBM 3624 validation data block carries: one Triple DES
/// block of two characters per byte.
const VALIDATION_DATA_BLOCK_CHARACTERS: usize = TDES_BLOCK_NIBBLES;

/// How many entries the modelled `DecimalizationTableType` carries: one per
/// hexadecimal character the encrypted validation data is read as.
const DECIMALIZATION_TABLE_ENTRIES: usize = 16;

/// The IBM 3624 natural PIN of `validation_data` under the PIN verification key
/// `key_algorithm` and `material` name, `length` digits long.
///
/// The published computation: the cardholder's validation data is padded on the right
/// with the pad character to one cipher block, encrypted under the key, and each
/// hexadecimal character of the result is replaced by the decimalization table's entry
/// at that character's value; the leading `length` digits are the natural PIN.
pub fn ibm3624_natural_pin(
    key_algorithm: &str,
    material: &[u8],
    validation_data: &str,
    pad_character: &str,
    decimalization_table: &str,
    length: usize,
) -> Pin<String> {
    if !(SHORTEST_PIN..=LONGEST_PIN).contains(&length) {
        return Err(PinRefusal::Invalid(format!(
            "A PIN of {length} digits is outside the {SHORTEST_PIN} to {LONGEST_PIN} digits a PIN \
             block carries."
        )));
    }
    let table = decimal_nibbles("DecimalizationTable", decimalization_table)?;
    if table.len() != DECIMALIZATION_TABLE_ENTRIES {
        return Err(PinRefusal::Invalid(format!(
            "DecimalizationTable carries {} digits, not {DECIMALIZATION_TABLE_ENTRIES}.",
            table.len()
        )));
    }
    if validation_data.len() > VALIDATION_DATA_BLOCK_CHARACTERS {
        return Err(PinRefusal::Invalid(format!(
            "PinValidationData carries {} characters, more than the \
             {VALIDATION_DATA_BLOCK_CHARACTERS} one validation data block holds.",
            validation_data.len()
        )));
    }
    let pad = hex_nibbles("PinValidationDataPadCharacter", pad_character)?;
    let Some(pad) = pad.first().copied() else {
        return Err(PinRefusal::Invalid(
            "PinValidationDataPadCharacter carries no character.".to_string(),
        ));
    };
    let mut block = hex_nibbles("PinValidationData", validation_data)?;
    block.resize(VALIDATION_DATA_BLOCK_CHARACTERS, pad);
    let mut state = bytes_of(&block);
    let cipher = verification_cipher(key_algorithm, material)?;
    cipher.encrypt_block(&mut state);
    let mut pin = String::with_capacity(length);
    for nibble in nibbles_of(&state).iter().take(length) {
        let entry = table[usize::from(*nibble)];
        let digit = char::from_digit(u32::from(entry), 10).ok_or_else(|| {
            PinRefusal::Invalid("DecimalizationTable carries no decimal digit.".to_string())
        })?;
        pin.push(digit);
    }
    Ok(pin)
}

/// The IBM 3624 offset of `pin` from `natural_pin`: the two digitwise, modulo ten.
///
/// The offset is what lets the PIN be recovered from a value stored in the clear, so it
/// is as long as the PIN it describes and the two are aligned digit for digit.
pub fn ibm3624_offset(pin: &str, natural_pin: &str) -> Pin<String> {
    digitwise(pin, natural_pin, |left, right| (left + 10 - right) % 10)
}

/// The IBM 3624 PIN of `offset` from `natural_pin` — the inverse of
/// [`ibm3624_offset`].
pub fn ibm3624_pin_from_offset(natural_pin: &str, offset: &str) -> Pin<String> {
    digitwise(natural_pin, offset, |left, right| (left + right) % 10)
}

/// `left` and `right` combined digit by digit by `combine`, modulo ten.
fn digitwise(left: &str, right: &str, combine: impl Fn(u8, u8) -> u8) -> Pin<String> {
    let left_digits = decimal_nibbles("PIN", left)?;
    let right_digits = decimal_nibbles("PinOffset", right)?;
    if left_digits.len() != right_digits.len() {
        return Err(PinRefusal::Invalid(format!(
            "A PIN of {} digits and an offset of {} digits are not the same length.",
            left_digits.len(),
            right_digits.len()
        )));
    }
    let mut combined = String::with_capacity(left_digits.len());
    for (left, right) in left_digits.iter().zip(right_digits.iter()) {
        let digit = combine(*left, *right);
        combined.push(char::from_digit(u32::from(digit), 10).unwrap_or('0'));
    }
    Ok(combined)
}

/// A PIN of `length` drawn digits — what the operations that GENERATE a PIN answer
/// with, rather than a PIN computed from the request, which would be a PIN the request
/// already knows.
pub fn drawn_pin(length: usize) -> String {
    let mut generator = ::rand::rng();
    let mut pin = String::with_capacity(length);
    for _ in 0..length {
        let digit: u32 = generator.random_range(0..10);
        pin.push(char::from_digit(digit, 10).unwrap_or('0'));
    }
    pin
}

/// The cipher a PIN verification key keys the IBM 3624 and Visa computations under.
///
/// Both are defined over a single Triple DES block, so a key whose algorithm names
/// another cipher — an AES key, an HMAC key — names no such computation.
fn verification_cipher(key_algorithm: &str, material: &[u8]) -> Pin<BlockCipher> {
    BlockCipher::new(key_algorithm, material)
        .filter(|cipher| cipher.block_len() == TDES_BLOCK_BYTES)
        .ok_or_else(|| {
            PinRefusal::Invalid(format!(
                "KeyAlgorithm {key_algorithm} is not valid for PIN data generation."
            ))
        })
}

/// `value`'s characters as decimal nibbles, or the refusal of a `member` that is not
/// decimal digits.
fn decimal_nibbles(member: &str, value: &str) -> Pin<Vec<u8>> {
    let mut nibbles = Vec::with_capacity(value.len());
    for character in value.chars() {
        let digit = character.to_digit(10).ok_or_else(|| {
            PinRefusal::Invalid(format!("{member} is not a string of decimal digits."))
        })?;
        nibbles.push(u8::try_from(digit).unwrap_or_default());
    }
    Ok(nibbles)
}

/// `value`'s characters as hexadecimal nibbles, or the refusal of a `member` that is
/// not hexadecimal.
fn hex_nibbles(member: &str, value: &str) -> Pin<Vec<u8>> {
    let mut nibbles = Vec::with_capacity(value.len());
    for character in value.chars() {
        let digit = character.to_digit(16).ok_or_else(|| {
            PinRefusal::Invalid(format!("{member} is not hexadecimal."))
        })?;
        nibbles.push(u8::try_from(digit).unwrap_or_default());
    }
    Ok(nibbles)
}

/// `nibbles`, most significant first, packed into bytes. A trailing odd nibble takes
/// the high half of its byte, which the callers here never produce: every field is a
/// whole number of bytes.
fn bytes_of(nibbles: &[u8]) -> Vec<u8> {
    nibbles
        .chunks(2)
        .map(|pair| (pair[0] << 4) | pair.get(1).copied().unwrap_or_default())
        .collect()
}

/// The inverse of [`bytes_of`].
fn nibbles_of(bytes: &[u8]) -> Vec<u8> {
    let mut nibbles = Vec::with_capacity(bytes.len() * 2);
    for byte in bytes {
        nibbles.push(byte >> 4);
        nibbles.push(byte & 0x0F);
    }
    nibbles
}

/// XOR `target` with `mask`, in place.
fn xor(target: &mut [u8], mask: &[u8]) {
    for (byte, mask) in target.iter_mut().zip(mask.iter()) {
        *byte ^= mask;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The working material of a double-length Triple DES key, as a key's own
    /// derivation hands it: sixteen bytes, which is what every computation here is
    /// defined over.
    fn tdes_material() -> Vec<u8> {
        hex::decode("0123456789ABCDEFFEDCBA9876543210").expect("the fixture is hexadecimal")
    }

    /// The working material of an AES-128 key, for the one format that uses one.
    fn aes_material() -> Vec<u8> {
        hex::decode("2B7E151628AED2A6ABF7158809CF4F3C").expect("the fixture is hexadecimal")
    }

    /// Every modelled `PinBlockFormatForPinData` value, with the key algorithm its
    /// block is defined over.
    fn formats() -> Vec<(PinBlockFormat, &'static str, Vec<u8>)> {
        vec![
            (PinBlockFormat::Iso0, "TDES_2KEY", tdes_material()),
            (PinBlockFormat::Iso1, "TDES_2KEY", tdes_material()),
            (PinBlockFormat::Iso3, "TDES_2KEY", tdes_material()),
            (PinBlockFormat::Iso4, "AES_128", aes_material()),
        ]
    }

    /// RULE: a PIN encrypted into a block of any modelled format is the PIN read back
    /// out of that block under the same key and account number, at every PIN length the
    /// model admits.
    #[test]
    fn a_pin_block_round_trips_at_every_modelled_pin_length() {
        let pan = "4111111111111111";
        for (format, key_algorithm, material) in formats() {
            for length in SHORTEST_PIN..=LONGEST_PIN {
                let pin: String = (1..=u32::try_from(length).expect("a modelled PIN length"))
                    .map(|index| char::from_digit(index % 10, 10).unwrap_or('0'))
                    .collect();
                let block =
                    encrypt_pin_block(format, key_algorithm, &material, &pin, Some(pan))
                        .unwrap_or_else(|refusal| {
                            panic!("case {format:?}/{length}: {refusal:?}")
                        });
                let read = decrypt_pin_block(format, key_algorithm, &material, &block, Some(pan))
                    .unwrap_or_else(|refusal| panic!("case {format:?}/{length}: {refusal:?}"));
                assert_eq!(
                    read, pin,
                    "case {format:?}/{length}: expected the PIN back, got {read:?}"
                );
            }
        }
    }

    /// RULE: the formats that bind the account number produce a block the PIN cannot be
    /// read out of under a different account number, and format 1, which binds none,
    /// reads the same either way.
    #[test]
    fn a_block_binds_the_account_number_of_every_format_but_one() {
        let pan = "4111111111111111";
        let other = "5500005555555559";
        for (format, key_algorithm, material) in formats() {
            let block = encrypt_pin_block(format, key_algorithm, &material, "1234", Some(pan))
                .unwrap_or_else(|refusal| panic!("case {format:?}: {refusal:?}"));
            let read = decrypt_pin_block(format, key_algorithm, &material, &block, Some(other));
            let same = read.as_ref().map(String::as_str).ok() == Some("1234");
            assert_eq!(
                same,
                !format.binds_account_number(),
                "case {format:?}: reading under another account number answered {read:?}"
            );
        }
    }

    /// RULE: the IBM 3624 offset is the digitwise difference modulo ten between the PIN
    /// and the natural PIN, so adding it back to the natural PIN returns the PIN.
    #[test]
    fn an_ibm_3624_offset_recovers_the_pin_it_was_computed_from() {
        let material = tdes_material();
        // (case, validation data, pad character, decimalization table, the PIN)
        let rows: &[(&str, &str, &str, &str, &str)] = &[
            (
                "the identity table",
                "4111111111111111",
                "F",
                "0123456789012345",
                "1234",
            ),
            (
                "a table that folds every character onto few digits",
                "123456",
                "F",
                "1111111111111111",
                "987654",
            ),
            (
                "validation data as long as the block",
                "9876543210987654",
                "0",
                "9876543210987654",
                "135790",
            ),
            (
                "the shortest validation data the model admits",
                "1234",
                "A",
                "0123456789012345",
                "246813579024",
            ),
        ];
        for (case, validation_data, pad, table, pin) in rows {
            let natural = ibm3624_natural_pin(
                "TDES_2KEY",
                &material,
                validation_data,
                pad,
                table,
                pin.len(),
            )
            .unwrap_or_else(|refusal| panic!("case {case:?}: {refusal:?}"));
            assert_eq!(
                natural.len(),
                pin.len(),
                "case {case:?}: expected a natural PIN of {} digits, got {natural:?}",
                pin.len()
            );
            let offset = ibm3624_offset(pin, &natural)
                .unwrap_or_else(|refusal| panic!("case {case:?}: {refusal:?}"));
            let recovered = ibm3624_pin_from_offset(&natural, &offset)
                .unwrap_or_else(|refusal| panic!("case {case:?}: {refusal:?}"));
            assert_eq!(
                recovered, *pin,
                "case {case:?}: expected the PIN back from its offset, got {recovered:?}"
            );
        }
    }

    /// RULE: a Visa PIN verification value is four decimal digits computed from the
    /// account number, the key index and the PIN, so changing any one of the three
    /// changes the value.
    #[test]
    fn a_visa_verification_value_is_four_digits_of_its_own_inputs() {
        let material = tdes_material();
        let value = |pan: &str, index: i32, pin: &str| {
            visa_pin_verification_value("TDES_2KEY", &material, pan, index, pin)
                .unwrap_or_else(|refusal| panic!("case {pan}/{index}/{}: {refusal:?}", pin.len()))
        };
        let base = value("4111111111111111", 1, "1234");
        assert_eq!(
            base.len(),
            PVV_DIGITS,
            "expected {PVV_DIGITS} digits, got {base:?}"
        );
        assert!(
            base.bytes().all(|byte| byte.is_ascii_digit()),
            "expected decimal digits, got {base:?}"
        );
        // (case, the value computed from one changed input)
        let rows: Vec<(&str, String)> = vec![
            ("another account number", value("5500005555555559", 1, "1234")),
            ("another key index", value("4111111111111111", 2, "1234")),
            ("another PIN", value("4111111111111111", 1, "4321")),
        ];
        for (case, other) in rows {
            assert_ne!(
                other, base,
                "case {case:?}: expected a different verification value, got {other:?}"
            );
        }
    }
}
