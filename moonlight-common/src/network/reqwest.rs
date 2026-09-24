use std::{sync::Arc, time::Duration};

use bytes::Bytes;
use log::debug;
use pem::Pem;
use reqwest::{Client, ClientBuilder};
use rustls::{
    CertificateError, DigitallySignedStruct, SignatureScheme,
    client::{
        Resumption,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
};
use thiserror::Error;
use url::{ParseError, Url};

use crate::network::{
    ApiError,
    request_client::{QueryParamsRef, RequestClient},
};

#[cfg(feature = "high")]
pub type ReqwestMoonlightHost = crate::high::MoonlightHost<reqwest::Client>;

#[derive(Debug, Error)]
pub enum ReqwestError {
    #[error("{0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("{0}")]
    UrlParse(#[from] ParseError),
    #[error("{0}")]
    Tls(#[from] rustls::Error),
}
pub type ReqwestApiError = ApiError<ReqwestError>;

fn default_builder() -> ClientBuilder {
    ClientBuilder::new()
        .use_native_tls()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(90))
        // https://github.com/seanmonstar/reqwest/issues/2021
        .pool_max_idle_per_host(0)
}
fn timeout_builder() -> ClientBuilder {
    default_builder().timeout(Duration::from_secs(2))
}

// Paired hosts are reached with rustls rather than native-tls. On Windows,
// native-tls imports the client private key into a persisted CryptoAPI key
// container named "native-tls-<n>", where <n> is a per-process counter
// starting at 0. web-server and every streamer process therefore write to the
// same container names, so starting a stream for one host silently replaced
// the key web-server used for another host, and all HTTPS requests to that
// host failed with SEC_E_DECRYPT_FAILURE until web-server was restarted.
// rustls keeps the key in process memory only.
fn build_client_with_certificates(
    builder: ClientBuilder,
    client_private_key: &Pem,
    client_certificate: &Pem,
    server_certificate: &Pem,
) -> Result<Client, ReqwestError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let client_key = match client_private_key.tag() {
        "RSA PRIVATE KEY" => PrivateKeyDer::Pkcs1(client_private_key.contents().to_vec().into()),
        _ => PrivateKeyDer::Pkcs8(client_private_key.contents().to_vec().into()),
    };
    let client_cert = CertificateDer::from(client_certificate.contents().to_vec());

    let verifier = PinnedServerCertVerifier {
        server_certificate: CertificateDer::from(server_certificate.contents().to_vec()),
        provider: provider.clone(),
    };

    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(vec![client_cert], client_key)?;
    // Sunshine (OpenSSL, client certificates required) answers resumption
    // attempts with an internal_error alert, so always do a full handshake.
    tls.resumption = Resumption::disabled();

    Ok(builder.use_preconfigured_tls(tls).build()?)
}

/// Trusts exactly the certificate the host presented during pairing.
///
/// Hosts use a self-signed certificate without a matching host name, so the
/// usual chain and name checks do not apply; pinning the certificate is the
/// same trust the previous native-tls setup expressed with a single root
/// certificate and `danger_accept_invalid_hostnames`. Handshake signatures
/// are still verified against the pinned certificate's key.
#[derive(Debug)]
struct PinnedServerCertVerifier {
    server_certificate: CertificateDer<'static>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.server_certificate.as_ref() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn build_url(
    use_https: bool,
    hostport: &str,
    path: &str,
    query_params: &QueryParamsRef<'_>,
) -> Result<Url, ReqwestError> {
    let protocol = if use_https { "https" } else { "http" };

    let authority = format!("{protocol}://{hostport}/{path}");
    let url = Url::parse_with_params(&authority, query_params)?;

    debug!("Request: {url}");

    Ok(url)
}

impl RequestClient for Client {
    type Error = ReqwestError;

    type Text = String;
    type Bytes = Bytes;

    fn with_defaults_long_timeout() -> Result<Self, Self::Error> {
        Ok(default_builder().build()?)
    }
    fn with_defaults() -> Result<Self, Self::Error> {
        Ok(timeout_builder().build()?)
    }

    fn with_certificates(
        client_private_key: &Pem,
        client_certificate: &Pem,
        server_certificate: &Pem,
    ) -> Result<Self, Self::Error> {
        build_client_with_certificates(
            timeout_builder(),
            client_private_key,
            client_certificate,
            server_certificate,
        )
    }

    fn with_certificates_long_timeout(
        client_private_key: &Pem,
        client_certificate: &Pem,
        server_certificate: &Pem,
    ) -> Result<Self, Self::Error> {
        build_client_with_certificates(
            default_builder(),
            client_private_key,
            client_certificate,
            server_certificate,
        )
    }

    async fn send_http_request_text_response(
        &mut self,
        hostport: &str,
        path: &str,
        query_params: &QueryParamsRef<'_>,
    ) -> Result<Self::Text, Self::Error> {
        let url = build_url(false, hostport, path, query_params)?;
        Ok(self.get(url).send().await?.text().await?)
    }

    async fn send_https_request_text_response(
        &mut self,
        hostport: &str,
        path: &str,
        query_params: &QueryParamsRef<'_>,
    ) -> Result<Self::Text, Self::Error> {
        let url = build_url(true, hostport, path, query_params)?;
        Ok(self.get(url).send().await?.text().await?)
    }

    async fn send_https_request_data_response(
        &mut self,
        hostport: &str,
        path: &str,
        query_params: &QueryParamsRef<'_>,
    ) -> Result<Self::Bytes, Self::Error> {
        let url = build_url(true, hostport, path, query_params)?;
        Ok(self.get(url).send().await?.bytes().await?)
    }
}
