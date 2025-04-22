use hkdf::Hkdf;
use sha2::Sha256;

pub struct CryptoHelper {
    aes_key: [u8; 32],
    iv: [u8; 16],
}

impl CryptoHelper {
    pub fn new(ikm: String) -> Self {
        let ikm = ikm.as_bytes();
        let hk = Hkdf::<Sha256>::new(None, ikm);
        
        todo!()
    }
}
