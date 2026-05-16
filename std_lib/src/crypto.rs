use crate::{OliveObj, olive_str_from_ptr, olive_str_internal};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::RngCore;
use rustc_hash::FxHashMap as HashMap;
use sha2::{Digest, Sha256};

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_sha256(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    let hash = Sha256::digest(text.as_bytes());
    olive_str_internal(&hex::encode(hash))
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_md5(s: i64) -> i64 {
    if s == 0 {
        return 0;
    }
    let text = olive_str_from_ptr(s);
    let hash = md5::compute(text.as_bytes());
    olive_str_internal(&format!("{:x}", hash))
}

fn key32_from_str(key_str: &str) -> [u8; 32] {
    let hash = Sha256::digest(key_str.as_bytes());
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&hash);
    arr
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_aes_encrypt(key_ptr: i64, data_ptr: i64) -> i64 {
    if key_ptr == 0 || data_ptr == 0 {
        return 0;
    }
    let key_str = olive_str_from_ptr(key_ptr);
    let data = olive_str_from_ptr(data_ptr);
    let key_bytes = key32_from_str(&key_str);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.encrypt(nonce, data.as_bytes()) {
        Ok(ciphertext) => {
            let mut combined = Vec::with_capacity(12 + ciphertext.len());
            combined.extend_from_slice(&nonce_bytes);
            combined.extend_from_slice(&ciphertext);
            olive_str_internal(&STANDARD.encode(&combined))
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_aes_decrypt(key_ptr: i64, data_ptr: i64) -> i64 {
    if key_ptr == 0 || data_ptr == 0 {
        return 0;
    }
    let key_str = olive_str_from_ptr(key_ptr);
    let data_b64 = olive_str_from_ptr(data_ptr);
    let combined = match STANDARD.decode(data_b64.as_bytes()) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    if combined.len() < 12 {
        return 0;
    }
    let key_bytes = key32_from_str(&key_str);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let nonce = Nonce::from_slice(&combined[..12]);
    match cipher.decrypt(nonce, &combined[12..]) {
        Ok(plaintext) => {
            let text = String::from_utf8_lossy(&plaintext).into_owned();
            olive_str_internal(&text)
        }
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_argon2_hash(password_ptr: i64) -> i64 {
    use argon2::{
        Argon2,
        password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
    };
    if password_ptr == 0 {
        return 0;
    }
    let password = olive_str_from_ptr(password_ptr);
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    match argon2.hash_password(password.as_bytes(), &salt) {
        Ok(hash) => olive_str_internal(&hash.to_string()),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_argon2_verify(password_ptr: i64, hash_ptr: i64) -> i64 {
    use argon2::{
        Argon2,
        password_hash::{PasswordHash, PasswordVerifier},
    };
    if password_ptr == 0 || hash_ptr == 0 {
        return 0;
    }
    let password = olive_str_from_ptr(password_ptr);
    let hash_str = olive_str_from_ptr(hash_ptr);
    let parsed = match PasswordHash::new(&hash_str) {
        Ok(h) => h,
        Err(_) => return 0,
    };
    if Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
    {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_rsa_keygen() -> i64 {
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
    use rsa::{RsaPrivateKey, RsaPublicKey};
    let mut rng = rand::thread_rng();
    let private_key = match RsaPrivateKey::new(&mut rng, 2048) {
        Ok(k) => k,
        Err(_) => return 0,
    };
    let public_key = RsaPublicKey::from(&private_key);
    let priv_der = match private_key.to_pkcs8_der() {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let pub_der = match public_key.to_public_key_der() {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let priv_b64 = STANDARD.encode(priv_der.as_bytes());
    let pub_b64 = STANDARD.encode(pub_der.as_bytes());
    let mut fields = HashMap::default();
    fields.insert("pub".to_string(), olive_str_internal(&pub_b64));
    fields.insert("priv".to_string(), olive_str_internal(&priv_b64));
    Box::into_raw(Box::new(OliveObj {
        kind: crate::KIND_OBJ,
        fields,
    })) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_rsa_encrypt(pub_ptr: i64, data_ptr: i64) -> i64 {
    use rsa::pkcs8::DecodePublicKey;
    use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
    if pub_ptr == 0 || data_ptr == 0 {
        return 0;
    }
    let pub_b64 = olive_str_from_ptr(pub_ptr);
    let data = olive_str_from_ptr(data_ptr);
    let pub_der = match STANDARD.decode(pub_b64.as_bytes()) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    let public_key = match RsaPublicKey::from_public_key_der(&pub_der) {
        Ok(k) => k,
        Err(_) => return 0,
    };
    let mut rng = rand::thread_rng();
    match public_key.encrypt(&mut rng, Pkcs1v15Encrypt, data.as_bytes()) {
        Ok(ciphertext) => olive_str_internal(&STANDARD.encode(&ciphertext)),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_crypto_rsa_decrypt(priv_ptr: i64, data_ptr: i64) -> i64 {
    use rsa::pkcs8::DecodePrivateKey;
    use rsa::{Pkcs1v15Encrypt, RsaPrivateKey};
    if priv_ptr == 0 || data_ptr == 0 {
        return 0;
    }
    let priv_b64 = olive_str_from_ptr(priv_ptr);
    let data_b64 = olive_str_from_ptr(data_ptr);
    let priv_der = match STANDARD.decode(priv_b64.as_bytes()) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    let private_key = match RsaPrivateKey::from_pkcs8_der(&priv_der) {
        Ok(k) => k,
        Err(_) => return 0,
    };
    let ciphertext = match STANDARD.decode(data_b64.as_bytes()) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    match private_key.decrypt(Pkcs1v15Encrypt, &ciphertext) {
        Ok(plaintext) => {
            let text = String::from_utf8_lossy(&plaintext).into_owned();
            olive_str_internal(&text)
        }
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olive_str_internal;

    fn s(text: &str) -> i64 {
        olive_str_internal(text)
    }

    fn from_ptr(ptr: i64) -> String {
        crate::olive_str_from_ptr(ptr)
    }

    #[test]
    fn sha256_empty() {
        let result = from_ptr(olive_crypto_sha256(s("")));
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hello() {
        let result = from_ptr(olive_crypto_sha256(s("hello")));
        assert_eq!(
            result,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_null_returns_zero() {
        assert_eq!(olive_crypto_sha256(0), 0);
    }

    #[test]
    fn sha256_length() {
        let result = from_ptr(olive_crypto_sha256(s("test")));
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn md5_hello() {
        let result = from_ptr(olive_crypto_md5(s("hello")));
        assert_eq!(result, "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn md5_null_returns_zero() {
        assert_eq!(olive_crypto_md5(0), 0);
    }

    #[test]
    fn md5_length() {
        let result = from_ptr(olive_crypto_md5(s("test")));
        assert_eq!(result.len(), 32);
    }

    #[test]
    fn aes_roundtrip() {
        let key = s("my-secret-key");
        let plaintext = s("hello, encrypted world!");
        let ciphertext = olive_crypto_aes_encrypt(key, plaintext);
        assert_ne!(ciphertext, 0);
        let decrypted = from_ptr(olive_crypto_aes_decrypt(key, ciphertext));
        assert_eq!(decrypted, "hello, encrypted world!");
    }

    #[test]
    fn aes_wrong_key_fails() {
        let key = s("correct-key");
        let wrong_key = s("wrong-key");
        let plaintext = s("secret");
        let ciphertext = olive_crypto_aes_encrypt(key, plaintext);
        assert_eq!(olive_crypto_aes_decrypt(wrong_key, ciphertext), 0);
    }

    #[test]
    fn aes_null_inputs() {
        assert_eq!(olive_crypto_aes_encrypt(0, s("data")), 0);
        assert_eq!(olive_crypto_aes_decrypt(s("key"), 0), 0);
    }

    #[test]
    fn argon2_hash_and_verify() {
        let pw = s("my_password_123");
        let hash = olive_crypto_argon2_hash(pw);
        assert_ne!(hash, 0);
        assert_eq!(olive_crypto_argon2_verify(pw, hash), 1);
        let wrong_pw = s("wrong_password");
        assert_eq!(olive_crypto_argon2_verify(wrong_pw, hash), 0);
    }

    #[test]
    fn argon2_null_inputs() {
        assert_eq!(olive_crypto_argon2_hash(0), 0);
        assert_eq!(olive_crypto_argon2_verify(0, s("hash")), 0);
    }

    #[test]
    fn rsa_keygen_returns_obj() {
        let obj_ptr = olive_crypto_rsa_keygen();
        assert_ne!(obj_ptr, 0);
        let obj = unsafe { &*(obj_ptr as *const OliveObj) };
        assert!(obj.fields.contains_key("pub"));
        assert!(obj.fields.contains_key("priv"));
        let pub_val = *obj.fields.get("pub").unwrap();
        let priv_val = *obj.fields.get("priv").unwrap();
        assert_ne!(pub_val, 0);
        assert_ne!(priv_val, 0);
    }

    #[test]
    fn rsa_encrypt_decrypt_roundtrip() {
        let obj_ptr = olive_crypto_rsa_keygen();
        let obj = unsafe { &*(obj_ptr as *const OliveObj) };
        let pub_key = *obj.fields.get("pub").unwrap();
        let priv_key = *obj.fields.get("priv").unwrap();
        let plaintext = s("secret message");
        let ciphertext = olive_crypto_rsa_encrypt(pub_key, plaintext);
        assert_ne!(ciphertext, 0);
        let decrypted = from_ptr(olive_crypto_rsa_decrypt(priv_key, ciphertext));
        assert_eq!(decrypted, "secret message");
    }
}
