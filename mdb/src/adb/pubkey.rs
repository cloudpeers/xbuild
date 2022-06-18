use anyhow::Result;
use byteorder::{WriteBytesExt, LE};
use num_bigint_dig::traits::ModInverse;
use num_bigint_dig::IntoBigInt;
use num_traits::ToPrimitive;
use rsa::{BigUint, PublicKeyParts, RsaPublicKey};
use std::io::{Cursor, Write};

#[derive(Debug, Eq, PartialEq)]
pub struct AndroidPublicKey {
    modulus_size_words: u32,
    /// Precomputed montgomery parameter: -1 / n[0] mod 2^32
    n0inv: u32,
    modulus: [u8; 256],
    /// Montgomery parameter R^2
    rr: [u8; 256],
    exponent: u32,
}

impl AndroidPublicKey {
    pub fn new(public: RsaPublicKey) -> Self {
        let mut modulus = [0; 256];
        let n = public.n().to_bytes_le();
        modulus.copy_from_slice(&n);

        let r32 = BigUint::from(1u8) << 32;
        let n0inv = public.n() % &r32;
        let n0inv = n0inv.mod_inverse(&r32).unwrap();
        let n0inv = r32.into_bigint().unwrap() - n0inv;
        let n0inv = n0inv.to_u32().unwrap();

        let rr = BigUint::from(1u8) << (256 * 8);
        let rr = (&rr * &rr) % public.n();
        let rr_bytes = rr.to_bytes_le();
        let mut rr = [0; 256];
        rr.copy_from_slice(&rr_bytes);

        Self {
            modulus_size_words: 64,
            n0inv,
            modulus,
            rr,
            exponent: public.e().to_u32().unwrap(),
        }
    }

    pub fn encode(&self) -> Result<String> {
        let mut buf = vec![0; 524];
        let mut c = Cursor::new(&mut buf);
        c.write_u32::<LE>(self.modulus_size_words)?;
        c.write_u32::<LE>(self.n0inv)?;
        c.write_all(&self.modulus)?;
        c.write_all(&self.rr)?;
        c.write_u32::<LE>(self.exponent)?;
        let mut res = base64::encode(&buf);
        res.push('\0');
        Ok(res)
    }

    #[cfg(test)]
    pub fn decode(public: &str) -> Result<Self> {
        use byteorder::ReadBytesExt;
        use std::io::Read;
        let buf = base64::decode(&public)?;
        let mut c = Cursor::new(buf);
        let modulus_size_words = c.read_u32::<LE>()?;
        let n0inv = c.read_u32::<LE>()?;
        let mut modulus = [0; 256];
        c.read_exact(&mut modulus)?;
        let mut rr = [0; 256];
        c.read_exact(&mut rr)?;
        let exponent = c.read_u32::<LE>()?;
        Ok(Self {
            modulus_size_words,
            n0inv,
            modulus,
            rr,
            exponent,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::adbkey;

    use super::*;

    pub fn adbpublickey() -> Result<AndroidPublicKey> {
        let home = dirs::home_dir().unwrap();
        let public_key2 = std::fs::read_to_string(home.join(".android/adbkey.pub"))?;
        let public_key2 = public_key2.split_once(' ').unwrap().0.to_string();
        AndroidPublicKey::decode(&public_key2)
    }

    #[test]
    fn test_public_key() -> Result<()> {
        let private_key = adbkey()?;
        let public_key = RsaPublicKey::from(&private_key);
        let public_key = AndroidPublicKey::new(public_key);

        let public_key2 = adbpublickey()?;
        assert_eq!(public_key2.n0inv, public_key.n0inv);
        assert_eq!(public_key2.modulus, public_key.modulus);
        assert_eq!(public_key2.rr, public_key.rr);
        assert_eq!(public_key2, public_key);
        Ok(())
    }
}
