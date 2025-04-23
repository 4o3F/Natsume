use anyhow::bail;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing_unwrap::OptionExt;


pub fn sync_info() -> anyhow::Result<()> {
    let base_url = &crate::GLOBAL_CONFIG
        .get()
        .unwrap_or_log()
        .client
        .server_addr;
    let parsed_url = reqwest::Url::parse(&base_url)
        .map_err(|_| anyhow::Error::msg("Failed to parse base URL"))?;
    let target_ip = parsed_url
        .host_str()
        .expect_or_log("Failed to get host str from base URL")
        .to_string();
    let mac = super::bind::get_mac(target_ip)?;
    tracing::info!("Current device MAC is {}", mac);

    let ikm = crate::GLOBAL_CONFIG
        .get()
        .unwrap_or_log()
        .client
        .key
        .clone();
    // let crypto_helper = crypto::CryptoHelper::new(ikm);

    // base64 encoded
    // let iv = crypto_helper.generate_iv()?;

    let request_url = format!("{}/sync", base_url);
    let client = reqwest::blocking::Client::new();

    #[derive(Serialize)]
    struct RequestBody {
        mac: String,
        iv: String,
    }

    // let body = RequestBody {
    //     mac: mac.clone(),
    //     iv: iv.clone(),
    // };

    // let response = client.post(request_url).json(&body).send()?;

    // match response.status() {
    //     StatusCode::OK => {}
    //     other => {
    //         let error: crate::client::ErrorResponse = response.json()?;
    //         tracing::error!("Wrong response code {}, error {}", other, error.msg);
    //         bail!("")
    //     }
    // }

    // Decrypt body
    // let body = response.text()?.trim().to_string();
    // tracing::info!("Encrypted body: {}, iv {}", body, iv.clone());
    // let decrypted = crypto_helper.decrypt(body, iv.clone())?;
    // #[derive(Deserialize, Debug)]
    // struct SyncResponseBody {
    //     username: String,
    //     password: String,
    // }
    // let body: SyncResponseBody = serde_json::from_str(decrypted.as_str())?;

    // tracing::info!("Synced info: {:?}", body);

    todo!()
}
