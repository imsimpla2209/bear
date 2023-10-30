use std::collections::HashMap;
use std::hash::Hash;
use anyhow::anyhow;
use serde::Deserialize;

use aws_sdk_ssm as aws;
use crate::errors::ApiError;

pub trait Cfg {
    fn server(&self) -> &ServerSettings;
}

#[derive(Deserialize, Clone, Default)]
pub struct ServerSettings {
    pub port: u16,
    pub oidc_client_id: String,
    pub public_url: String,
    pub oidc_admins: Vec<String>,
    pub ssm_prefix: String,
}

pub trait SsmKeyTrait: strum::IntoEnumIterator + Eq + Hash {
    fn key(&self) -> &str;
}

pub struct SecretsContainer<T: SsmKeyTrait>(pub HashMap<T, String>);

impl<T: SsmKeyTrait> SecretsContainer<T> {
    pub async fn load(aws_ssm: &Option<AwsClient>, prefix: &str) -> anyhow::Result<Self> {
        let mut secrets = HashMap::new();
        for key in T::iter() {
            let value = smps_read(aws_ssm, &format!("{}/{}", prefix, key.key())).await?;
            secrets.insert(key, value);
        }
        Ok(Self(secrets))
    }

    pub fn get(&self, key: T) -> &str {
        self.0.get(&key).unwrap()
    }
}

pub type AwsClient = aws::Client;

pub async fn aws_ssm_client(dev_mode: bool) -> Option<AwsClient> {
    let mut builder = aws_config::from_env();
    if dev_mode {
        builder = builder.profile_name("earn_backend_dev")
    }
    let shared_config = builder.load().await;
    if shared_config.region().is_none() { return None };
    let cl = aws::Client::new(&shared_config);
    Some(cl)
}

pub async fn smps_read(aws_ssm: &Option<AwsClient>, param_name: &str) -> anyhow::Result<String> {
    if let Some(aws_ssm_it) = aws_ssm {
        let param_result = aws_ssm_it.get_parameter()
            .name(param_name)
            .with_decryption(true)
            .send().await?;
        Ok(param_result
            .parameter().ok_or(anyhow!("parameter missing from SSM response"))?
            .value().ok_or(anyhow!("parameter value missing from SSM response"))?
            .into())
    } else {
        Err(ApiError::Disabled.into())
    }
}
