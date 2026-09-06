use once_cell::sync::Lazy;

/// Apple's `sylvan.apple.com` (the Aerial manifest/video CDN) serves a chain
/// signed by "Apple Root CA" -- a long-standing, publicly documented Apple
/// root (see <https://www.apple.com/certificateauthority/>) that predates
/// most public CA programs and was never submitted to Mozilla's/most Linux
/// distros' root stores, since it's mainly used for Apple's own internal
/// services rather than the public web. Neither the system trust store nor
/// rustls's bundled webpki-roots trust it, so plain TLS verification fails
/// with `UnknownIssuer` even though the connection is genuinely to Apple.
///
/// Rather than disabling certificate verification (which would accept any
/// server), we pin the specific intermediate CA that signs this endpoint --
/// captured directly from Apple's own TLS handshake -- as an additional
/// trusted root for our client only. This keeps verification strict (a
/// MITM'd or spoofed cert still fails) while allowing this one legitimate
/// Apple service to validate.
const APPLE_SERVER_AUTHENTICATION_CA_PEM: &[u8] =
    include_bytes!("../assets/apple-server-authentication-ca.pem");

pub static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    let apple_ca = reqwest::Certificate::from_pem(APPLE_SERVER_AUTHENTICATION_CA_PEM)
        .expect("bundled Apple Server Authentication CA cert must be valid PEM");

    reqwest::Client::builder()
        .add_root_certificate(apple_ca)
        .build()
        .expect("failed to build shared reqwest client")
});
