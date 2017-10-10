// Copyright 2016 The Grin Developers
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

use api;
use checker;
use core::core::{Transaction, build};
use core::ser;
use keychain::{BlindingFactor, Keychain, Fingerprint};
use receiver::TxWrapper;
use types::*;
use util;

use secp;

/// Issue a new transaction to the provided sender by spending some of our
/// wallet
/// UTXOs. The destination can be "stdout" (for command line) or a URL to the
/// recipients wallet receiver (to be implemented).
pub fn issue_send_tx(
	config: &WalletConfig,
	keychain: &Keychain,
	amount: u64,
	dest: String,
) -> Result<(), Error> {
	checker::refresh_outputs(config, keychain)?;

	let chain_tip = checker::get_tip_from_node(config)?;
	let lock_height = chain_tip.height;

	let (tx, blind_sum) = build_send_tx(config, keychain, amount, lock_height)?;
	let json_tx = partial_tx_to_json(amount, blind_sum, tx);

	if dest == "stdout" {
		println!("{}", json_tx);
	} else if &dest[..4] == "http" {
		let url = format!("{}/v1/receive/receive_json_tx", &dest);
		debug!("Posting partial transaction to {}", url);
		let request = WalletReceiveRequest::PartialTransaction(json_tx);
		let _: CbData = api::client::post(url.as_str(), &request)
			.expect(&format!("Wallet receiver at {} unreachable, could not send transaction. Is it running?", url));
	} else {
		panic!("dest not in expected format: {}", dest);
	}
	Ok(())
}

/// Builds a transaction to send to someone from the HD seed associated with the
/// wallet and the amount to send. Handles reading through the wallet data file,
/// selecting outputs to spend and building the change.
fn build_send_tx(
	config: &WalletConfig,
	keychain: &Keychain,
	amount: u64,
	lock_height: u64,
) -> Result<(Transaction, BlindingFactor), Error> {
	let fingerprint = keychain.clone().fingerprint();

	// operate within a lock on wallet data
	WalletData::with_wallet(&config.data_file_dir, |wallet_data| {

		// select some suitable outputs to spend from our local wallet
		let (coins, change) = wallet_data.select(fingerprint.clone(), amount);
		if change < 0 {
			return Err(Error::NotEnoughFunds((-change) as u64));
		}

		// build transaction skeleton with inputs and change
		let parts = inputs_and_change(&coins, keychain, fingerprint, wallet_data, amount)?;

		// This is more proof of concept than anything but here we set a
		// lock_height on the transaction being sent (based on current chain height via api).
		parts.push(build::with_lock_height(lock_height));

		let result = build::transaction(parts, &keychain)?;
		Ok(result)
	})?
}

pub fn issue_burn_tx(
	config: &WalletConfig,
	keychain: &Keychain,
	amount: u64,
) -> Result<(), Error> {

	let _ = checker::refresh_outputs(config, keychain);
	let fingerprint = keychain.clone().fingerprint();
	let sk_burn = secp::key::SecretKey::from_slice(keychain.secp(), &[1, 32])?;

	// operate within a lock on wallet data
	WalletData::with_wallet(&config.data_file_dir, |mut wallet_data| {
		
		// select all suitable outputs by passing largest amount
		let (coins, _) = wallet_data.select(fingerprint.clone(), amount);

		// build transaction skeleton with inputs and change
		let mut parts = inputs_and_change(&coins, keychain, fingerprint, &mut wallet_data, amount)?;

		// add burn output and fees
		parts.push(build::output_raw(amount, sk_burn));

		// finalize the burn transaction and send
		let (tx_burn, _) = build::transaction(parts, &keychain)?;
		tx_burn.validate(&keychain.secp())?;

		let tx_hex = util::to_hex(ser::ser_vec(&tx_burn).unwrap());
		let url = format!("{}/v1/pool/push", config.check_node_api_http_addr.as_str());
		let _: () = api::client::post(url.as_str(), &TxWrapper { tx_hex: tx_hex })
			.map_err(|e| Error::Node(e))?;
		Ok(())
	})?
}

fn inputs_and_change(coins: &Vec<OutputData>, keychain: &Keychain, fingerprint: Fingerprint, wallet_data: &mut WalletData, amount: u64) -> Result<Vec<Box<build::Append>>, Error> {

	let mut parts = vec![];

	// calculate the total in inputs, fees and how much is left
	let total: u64 = coins.iter().map(|c| c.value).sum();
	let fee = tx_fee(coins.len(), 2, None);
	let shortage = (total as i64) - (amount as i64) - (fee as i64);
	if shortage < 0 {
		return Err(Error::NotEnoughFunds((-shortage) as u64));
	}
	parts.push(build::with_fee(fee));
	let change = total - amount - fee;

	// build inputs using the appropriate derived pubkeys
	for coin in coins {
		let pubkey = keychain.derive_pubkey(coin.n_child)?;
		parts.push(build::input(coin.value, pubkey));
	}

	// derive an additional pubkey for change and build the change output
	let change_derivation = wallet_data.next_child(fingerprint.clone());
	let change_key = keychain.derive_pubkey(change_derivation)?;
	parts.push(build::output(change, change_key.clone()));
	
	// we got that far, time to start tracking the new output
	// and lock the outputs used
	wallet_data.add_output(OutputData {
		fingerprint: fingerprint.clone(),
		identifier: change_key.clone(),
		n_child: change_derivation,
		value: change as u64,
		status: OutputStatus::Unconfirmed,
		height: 0,
		lock_height: 0,
	});

	// lock the ouputs we're spending
	for coin in coins {
		wallet_data.lock_output(coin);
	}

	Ok(parts)
}

#[cfg(test)]
mod test {
	use core::core::build::{input, output, transaction};
	use keychain::Keychain;

	#[test]
	// demonstrate that input.commitment == referenced output.commitment
	// based on the public key and amount begin spent
	fn output_commitment_equals_input_commitment_on_spend() {
		let keychain = Keychain::from_random_seed().unwrap();
		let pk1 = keychain.derive_pubkey(1).unwrap();

		let (tx, _) = transaction(
			vec![output(105, pk1.clone())],
			&keychain,
		).unwrap();

		let (tx2, _) = transaction(
			vec![input(105, pk1.clone())],
			&keychain,
		).unwrap();

		assert_eq!(tx.outputs[0].commitment(), tx2.inputs[0].commitment());
	}
}
