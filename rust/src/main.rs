use bitcoincore_rpc::{Auth, Client};
use serde_json::{json, Value};
use std::error::Error;
use std::fs::File;
use std::io::{Error as IoError, ErrorKind, Write};

const RPC_URL: &str = "http://127.0.0.1:18443";
const RPC_USER: &str = "alice";
const RPC_PASS: &str = concat!("pass", "word");

fn boxed_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(IoError::new(ErrorKind::Other, message.into()))
}

fn wallet_url(wallet_name: &str) -> String {
    format!("{}/wallet/{}", RPC_URL, wallet_name)
}

fn wallet_client(wallet_name: &str) -> Result<Client, Box<dyn Error>> {
    Ok(Client::new(
        &wallet_url(wallet_name),
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?)
}

fn ensure_wallet(base_rpc: &Client, wallet_name: &str) -> Result<(), Box<dyn Error>> {
    let loaded_wallets: Vec<String> = base_rpc.call("listwallets", &[])?;
    if loaded_wallets.iter().any(|name| name == wallet_name) {
        return Ok(());
    }

    if base_rpc
        .call::<Value>("createwallet", &[json!(wallet_name)])
        .is_err()
    {
        base_rpc.call::<Value>("loadwallet", &[json!(wallet_name)])?;
    }

    Ok(())
}

fn rpc_string(rpc: &Client, method: &str, args: &[Value]) -> Result<String, Box<dyn Error>> {
    Ok(rpc.call::<String>(method, args)?)
}

fn rpc_value(rpc: &Client, method: &str, args: &[Value]) -> Result<Value, Box<dyn Error>> {
    Ok(rpc.call::<Value>(method, args)?)
}

fn value_as_f64(value: &Value, label: &str) -> Result<f64, Box<dyn Error>> {
    value
        .as_f64()
        .ok_or_else(|| boxed_error(format!("Expected {label} to be a number")))
}

fn value_as_str(value: &Value, label: &str) -> Result<String, Box<dyn Error>> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| boxed_error(format!("Expected {label} to be a string")))
}

fn get_balance(wallet_rpc: &Client) -> Result<f64, Box<dyn Error>> {
    let balance = rpc_value(wallet_rpc, "getbalance", &[])?;
    value_as_f64(&balance, "wallet balance")
}

fn mine_until_spendable(miner_rpc: &Client, mining_address: &str) -> Result<u64, Box<dyn Error>> {
    // Coinbase rewards are immature at first. On regtest, the first block reward becomes
    // spendable only after enough maturity confirmations, so this usually takes 101 blocks.
    for blocks_mined in 0..150_u64 {
        if get_balance(miner_rpc)? > 0.0 {
            return Ok(blocks_mined);
        }

        rpc_value(
            miner_rpc,
            "generatetoaddress",
            &[json!(1), json!(mining_address)],
        )?;
    }

    Err(boxed_error(
        "Mining did not produce a spendable balance after 150 blocks",
    ))
}

fn output_address(output: &Value) -> Result<String, Box<dyn Error>> {
    value_as_str(
        &output["scriptPubKey"]["address"],
        "transaction output address",
    )
}

fn output_value(output: &Value) -> Result<f64, Box<dyn Error>> {
    value_as_f64(&output["value"], "transaction output amount")
}

fn main() -> Result<(), Box<dyn Error>> {
    let base_rpc = Client::new(
        RPC_URL,
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?;

    ensure_wallet(&base_rpc, "Miner")?;
    ensure_wallet(&base_rpc, "Trader")?;

    let miner_rpc = wallet_client("Miner")?;
    let trader_rpc = wallet_client("Trader")?;

    let mining_address = rpc_string(
        &miner_rpc,
        "getnewaddress",
        &[json!("Mining Reward"), json!("bech32")],
    )?;

    let blocks_mined = mine_until_spendable(&miner_rpc, &mining_address)?;
    println!(
        "Mined {blocks_mined} blocks before the Miner wallet had a spendable balance. Coinbase rewards need maturity confirmations before they can be spent."
    );

    let miner_balance = get_balance(&miner_rpc)?;
    println!("Miner balance: {miner_balance} BTC");

    let trader_address = rpc_string(
        &trader_rpc,
        "getnewaddress",
        &[json!("Received"), json!("bech32")],
    )?;

    let txid = rpc_string(
        &miner_rpc,
        "sendtoaddress",
        &[json!(trader_address), json!(20.0)],
    )?;

    let mempool_entry = rpc_value(&base_rpc, "getmempoolentry", &[json!(txid)])?;
    println!("Unconfirmed mempool entry: {mempool_entry:#}");

    rpc_value(
        &miner_rpc,
        "generatetoaddress",
        &[json!(1), json!(mining_address)],
    )?;

    let tx = rpc_value(
        &miner_rpc,
        "gettransaction",
        &[json!(txid), json!(null), json!(true)],
    )?;
    let decoded = &tx["decoded"];
    let vin = decoded["vin"]
        .as_array()
        .and_then(|inputs| inputs.first())
        .ok_or_else(|| boxed_error("Expected the transaction to have one input"))?;

    let previous_txid = value_as_str(&vin["txid"], "previous transaction id")?;
    let previous_vout_index = vin["vout"]
        .as_u64()
        .ok_or_else(|| boxed_error("Expected previous vout index"))?
        as usize;

    let previous_tx = rpc_value(
        &miner_rpc,
        "gettransaction",
        &[json!(previous_txid), json!(null), json!(true)],
    )?;
    let previous_output = previous_tx["decoded"]["vout"]
        .as_array()
        .and_then(|outputs| outputs.get(previous_vout_index))
        .ok_or_else(|| boxed_error("Could not read the previous output spent by Miner"))?;

    let miner_input_address = output_address(previous_output)?;
    let miner_input_amount = output_value(previous_output)?;

    let outputs = decoded["vout"]
        .as_array()
        .ok_or_else(|| boxed_error("Expected decoded transaction outputs"))?;

    let mut trader_output_address = String::new();
    let mut trader_output_amount = 0.0;
    let mut miner_change_address = String::new();
    let mut miner_change_amount = 0.0;

    for output in outputs {
        let address = output_address(output)?;
        let amount = output_value(output)?;

        if address == trader_address {
            trader_output_address = address;
            trader_output_amount = amount;
        } else {
            miner_change_address = address;
            miner_change_amount = amount;
        }
    }

    if trader_output_address.is_empty() || miner_change_address.is_empty() {
        return Err(boxed_error(
            "Could not identify both the Trader output and Miner change output",
        ));
    }

    let fee = value_as_f64(&tx["fee"], "transaction fee")?.abs();
    let block_height = tx["blockheight"]
        .as_i64()
        .ok_or_else(|| boxed_error("Expected confirmed transaction block height"))?;
    let block_hash = value_as_str(&tx["blockhash"], "confirmed transaction block hash")?;

    let mut file = File::create("../out.txt")?;
    writeln!(file, "{txid}")?;
    writeln!(file, "{miner_input_address}")?;
    writeln!(file, "{miner_input_amount}")?;
    writeln!(file, "{trader_output_address}")?;
    writeln!(file, "{trader_output_amount}")?;
    writeln!(file, "{miner_change_address}")?;
    writeln!(file, "{miner_change_amount}")?;
    writeln!(file, "{fee}")?;
    writeln!(file, "{block_height}")?;
    writeln!(file, "{block_hash}")?;

    Ok(())
}
