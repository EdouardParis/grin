// Copyright 2017 The Grin Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{error, fmt};
use std::cmp::min;

use serde::{de, ser};

use byteorder::{ByteOrder, BigEndian};
use blake2::blake2b::blake2b;
use secp;
use secp::Secp256k1;
use secp::key::{PublicKey, SecretKey};
use util;

/// An ExtKey error
#[derive(Copy, PartialEq, Eq, Clone, Debug)]
pub enum Error {
	/// The size of the seed is invalid
	InvalidSeedSize,
	InvalidSliceSize,
	InvalidExtendedKey,
	Secp(secp::Error),
}

impl From<secp::Error> for Error {
	fn from(e: secp::Error) -> Error {
		Error::Secp(e)
	}
}

// Passthrough Debug to Display, since errors should be user-visible
impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		f.write_str(error::Error::description(self))
	}
}

impl error::Error for Error {
	fn cause(&self) -> Option<&error::Error> {
		None
	}

	fn description(&self) -> &str {
		match *self {
			Error::InvalidSeedSize => "keychain: seed isn't of size 128, 256 or 512",
			// TODO change when ser. ext. size is fixed
			Error::InvalidSliceSize => "keychain: serialized extended key must be of size 73",
			Error::InvalidExtendedKey => "keychain: the given serialized extended key is invalid",
			Error::Secp(_) => "keychain: secp error",
		}
	}
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
	fn zero() -> Fingerprint {
		Identifier::from_bytes(&[0; 4]).fingerprint()
	}

	fn from_bytes(bytes: &[u8]) -> Fingerprint {
		let mut fingerprint = [0; 4];
		for i in 0..min(4, bytes.len()) {
			fingerprint[i] = bytes[i];
		}
		Fingerprint(util::to_hex(fingerprint.to_vec()))
	}
}

impl fmt::Display for Fingerprint {
	fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		f.write_str(&self.0)
	}
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Identifier([u8; 20]);

impl ser::Serialize for Identifier {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: ser::Serializer,
	{
		serializer.serialize_str(&self.to_hex())
	}
}

impl<'de> de::Deserialize<'de> for Identifier {
	fn deserialize<D>(deserializer: D) -> Result<Identifier, D::Error>
	where
		D: de::Deserializer<'de>,
	{
		deserializer.deserialize_u64(IdentifierVisitor)
	}
}

struct IdentifierVisitor;

impl<'de> de::Visitor<'de> for IdentifierVisitor {
	type Value = Identifier;

	fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
		formatter.write_str("an identifier")
	}

	fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
	where
		E: de::Error,
	{
		// TODO - error handling here
		let identifier = Identifier::from_hex(s).unwrap();
		Ok(identifier)
	}
}

impl Identifier {
	pub fn from_bytes(bytes: &[u8]) -> Identifier {
		let mut identifier = [0; 20];
		for i in 0..min(20, bytes.len()) {
			identifier[i] = bytes[i];
		}
		Identifier(identifier)
	}

	pub fn from_pubkey(secp: &Secp256k1, pubkey: &PublicKey) -> Identifier {
		let bytes = pubkey.serialize_vec(secp, true);
		let identifier = blake2b(20, &[], &bytes[..]);
		Identifier::from_bytes(&identifier.as_bytes())
	}

	fn from_hex(hex: &str) -> Result<Identifier, Error> {
		// TODO - error handling, don't unwrap here
		let bytes = util::from_hex(hex.to_string()).unwrap();
		Ok(Identifier::from_bytes(&bytes))
	}

	pub fn to_hex(&self) -> String {
		util::to_hex(self.0.to_vec())
	}

	pub fn fingerprint(&self) -> Fingerprint {
		Fingerprint::from_bytes(&self.0)
	}
}

impl AsRef<[u8]> for Identifier {
	fn as_ref(&self) -> &[u8] {
		&self.0.as_ref()
	}
}

impl ::std::fmt::Debug for Identifier {
	fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
		try!(write!(f, "{}(", stringify!(Identifier)));
		for i in self.0.iter().cloned() {
			try!(write!(f, "{:02x}", i));
		}
		write!(f, ")")
	}
}

/// An ExtendedKey is a secret key which can be used to derive new
/// secret keys to blind the commitment of a transaction output.
/// To be usable, a secret key should have an amount assigned to it,
/// but when the key is derived, the amount is not known and must be
/// given.
#[derive(Debug, Clone)]
pub struct ExtendedKey {
	/// Depth of the extended key
	pub depth: u8,
	/// Child number of the key
	pub n_child: u32,
	/// Parent key's fingerprint
	pub fingerprint: Fingerprint,
	/// Code of the derivation chain
	pub chaincode: [u8; 32],
	/// Actual private key
	pub key: SecretKey,
}

impl ExtendedKey {
	/// Creates a new extended key from a serialized one
	pub fn from_slice(secp: &Secp256k1, slice: &[u8]) -> Result<ExtendedKey, Error> {
		// TODO change when ser. ext. size is fixed
		if slice.len() != 73 {
			return Err(Error::InvalidSliceSize);
		}
		let depth: u8 = slice[0];
		let fingerprint = Fingerprint::from_bytes(&slice[1..5]);
		let n_child = BigEndian::read_u32(&slice[5..9]);
		let mut chaincode: [u8; 32] = [0; 32];
		(&mut chaincode).copy_from_slice(&slice[9..41]);
		let secret_key = match SecretKey::from_slice(secp, &slice[41..73]) {
			Ok(key) => key,
			Err(_) => return Err(Error::InvalidExtendedKey),
		};

		Ok(ExtendedKey {
			depth: depth,
			fingerprint: fingerprint,
			n_child: n_child,
			chaincode: chaincode,
			key: secret_key,
		})
	}

	/// Creates a new extended master key from a seed
	pub fn from_seed(secp: &Secp256k1, seed: &[u8]) -> Result<ExtendedKey, Error> {
		match seed.len() {
			16 | 32 | 64 => (),
			_ => return Err(Error::InvalidSeedSize),
		}

		let derived = blake2b(64, b"Mimble seed", seed);

		let mut chaincode: [u8; 32] = [0; 32];
		(&mut chaincode).copy_from_slice(&derived.as_bytes()[32..]);
		// TODO Error handling
		let secret_key = SecretKey::from_slice(&secp, &derived.as_bytes()[0..32])
			.expect("Error generating from seed");

		let mut ext_key = ExtendedKey {
			depth: 0,
			fingerprint: Fingerprint::zero(),
			n_child: 0,
			chaincode: chaincode,
			key: secret_key,
		};

		let identifier = ext_key.identifier(secp)?;
		ext_key.fingerprint = identifier.fingerprint();

		Ok(ext_key)
	}

	/// Return the identifier of the key
	/// which is the blake2b hash (20 byte digest) of the PublicKey
	// corresponding to the underlying SecretKey
	pub fn identifier(&self, secp: &Secp256k1) -> Result<Identifier, Error> {
		let pubkey = PublicKey::from_secret_key(secp, &self.key)?;
		Ok(Identifier::from_pubkey(secp, &pubkey))
	}

	/// Derive an extended key from an extended key
	pub fn derive(&self, secp: &Secp256k1, n: u32) -> Result<ExtendedKey, Error> {
		let mut n_bytes: [u8; 4] = [0; 4];
		BigEndian::write_u32(&mut n_bytes, n);
		let mut seed = self.key[..].to_vec();
		seed.extend_from_slice(&n_bytes);

		let derived = blake2b(64, &self.chaincode[..], &seed[..]);

		let mut secret_key = SecretKey::from_slice(&secp, &derived.as_bytes()[0..32])
			.expect("Error deriving key");
		secret_key.add_assign(secp, &self.key).expect(
			"Error deriving key",
		);
		// TODO check if key != 0 ?

		let mut chain_code: [u8; 32] = [0; 32];
		(&mut chain_code).clone_from_slice(&derived.as_bytes()[32..]);

		let identifier = self.identifier(&secp)?;

		Ok(ExtendedKey {
			depth: self.depth + 1,
			fingerprint: identifier.fingerprint(),
			n_child: n,
			chaincode: chain_code,
			key: secret_key,
		})
	}
}

#[cfg(test)]
mod test {
	use serde_json;

	use secp::Secp256k1;
	use secp::key::SecretKey;
	use super::{ExtendedKey, Fingerprint, Identifier};
	use util;

	fn from_hex(hex_str: &str) -> Vec<u8> {
		util::from_hex(hex_str.to_string()).unwrap()
	}

	#[test]
	fn test_identifier_json_ser_deser() {
		let hex = "942b6c0bd43bdcb24f3edfe7fadbc77054ecc4f2";
		let identifier = Identifier::from_hex(hex).unwrap();

		#[derive(Debug, Serialize, Deserialize, PartialEq)]
		struct HasAnIdentifier {
			identifier: Identifier,
		}

		let has_an_identifier = HasAnIdentifier { identifier };

		let json = serde_json::to_string(&has_an_identifier).unwrap();

		assert_eq!(
			json,
			"{\"identifier\":\"942b6c0bd43bdcb24f3edfe7fadbc77054ecc4f2\"}"
		);

		let deserialized: HasAnIdentifier = serde_json::from_str(&json).unwrap();
		assert_eq!(deserialized, has_an_identifier);
	}

	#[test]
	fn extkey_from_seed() {
		// TODO More test vectors
		let s = Secp256k1::new();
		let seed = from_hex("000102030405060708090a0b0c0d0e0f");
		let extk = ExtendedKey::from_seed(&s, &seed.as_slice()).unwrap();
		let sec = from_hex(
			"c3f5ae520f474b390a637de4669c84d0ed9bbc21742577fac930834d3c3083dd",
		);
		let secret_key = SecretKey::from_slice(&s, sec.as_slice()).unwrap();
		let chaincode = from_hex(
			"e7298e68452b0c6d54837670896e1aee76b118075150d90d4ee416ece106ae72",
		);
		let identifier = from_hex("d291fc2dca90fc8b005a01638d616fda770ec552");
		let fingerprint = from_hex("d291fc2d");
		let depth = 0;
		let n_child = 0;
		assert_eq!(extk.key, secret_key);
		assert_eq!(
			extk.identifier(&s).unwrap(),
			Identifier::from_bytes(identifier.as_slice())
		);
		assert_eq!(
			extk.fingerprint,
			Fingerprint::from_bytes(fingerprint.as_slice())
		);
		assert_eq!(
			extk.identifier(&s).unwrap().fingerprint(),
			Fingerprint::from_bytes(fingerprint.as_slice())
		);
		assert_eq!(extk.chaincode, chaincode.as_slice());
		assert_eq!(extk.depth, depth);
		assert_eq!(extk.n_child, n_child);
	}

	#[test]
	fn extkey_derivation() {
		// TODO More test vectors
		let s = Secp256k1::new();
		let seed = from_hex("000102030405060708090a0b0c0d0e0f");
		let extk = ExtendedKey::from_seed(&s, &seed.as_slice()).unwrap();
		let derived = extk.derive(&s, 0).unwrap();
		let sec = from_hex(
			"d75f70beb2bd3b56f9b064087934bdedee98e4b5aae6280c58b4eff38847888f",
		);
		let secret_key = SecretKey::from_slice(&s, sec.as_slice()).unwrap();
		let chaincode = from_hex(
			"243cb881e1549e714db31d23af45540b13ad07941f64a786bbf3313b4de1df52",
		);
		let fingerprint = from_hex("d291fc2d");
		let identifier = from_hex("027a8e290736af382fc943bdabb774bc2d14fd95");
		let identifier_fingerprint = from_hex("027a8e29");
		let depth = 1;
		let n_child = 0;
		assert_eq!(derived.key, secret_key);
		assert_eq!(
			derived.identifier(&s).unwrap(),
			Identifier::from_bytes(identifier.as_slice())
		);
		assert_eq!(
			derived.fingerprint,
			Fingerprint::from_bytes(fingerprint.as_slice())
		);
		assert_eq!(
			derived.identifier(&s).unwrap().fingerprint(),
			Fingerprint::from_bytes(identifier_fingerprint.as_slice())
		);
		assert_eq!(derived.chaincode, chaincode.as_slice());
		assert_eq!(derived.depth, depth);
		assert_eq!(derived.n_child, n_child);
	}
}
