use std;
use std::io::{Read, Write};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

#[cfg(not(target_arch = "wasm32"))]
use crypto::{aes, buffer, blockmodes::NoPadding};

#[cfg(all(target_arch = "wasm32"))]
use aes_soft::{Aes256, BlockCipher, block_cipher_trait::generic_array::GenericArray};

use message_types::{SrdMac, SrdMessage, srd_flags::SRD_FLAG_MAC, srd_msg_id::SRD_DELEGATE_MSG_ID,
                    SRD_SIGNATURE};
use Result;

pub struct SrdDelegate {
    pub signature: u32,
    pub packet_type: u8,
    pub seq_num: u8,
    pub flags: u16,
    pub size: u32,
    pub blob: Vec<u8>,
    pub mac: [u8; 32],
}

impl SrdMessage for SrdDelegate {
    fn read_from(buffer: &mut std::io::Cursor<Vec<u8>>) -> Result<Self>
    where
        Self: Sized,
    {
        let signature = buffer.read_u32::<LittleEndian>()?;
        let packet_type = buffer.read_u8()?;
        let seq_num = buffer.read_u8()?;
        let flags = buffer.read_u16::<LittleEndian>()?;
        let size = buffer.read_u32::<LittleEndian>()?;

        let mut blob = vec![0u8; size as usize];
        buffer.read_exact(&mut blob)?;

        let mut mac = [0u8; 32];

        buffer.read_exact(&mut mac)?;

        Ok(SrdDelegate {
            signature,
            packet_type,
            seq_num,
            flags,
            size,
            blob,
            mac,
        })
    }

    fn write_to(&self, mut buffer: &mut Vec<u8>) -> Result<()> {
        self.write_inner_buffer(&mut buffer)?;
        buffer.write_all(&self.mac)?;
        Ok(())
    }

    fn get_id(&self) -> u8 {
        SRD_DELEGATE_MSG_ID
    }
}

impl SrdMac for SrdDelegate {
    fn write_inner_buffer(&self, buffer: &mut Vec<u8>) -> Result<()> {
        buffer.write_u32::<LittleEndian>(self.signature)?;
        buffer.write_u8(self.packet_type)?;
        buffer.write_u8(self.seq_num)?;
        buffer.write_u16::<LittleEndian>(self.flags)?;
        buffer.write_u32::<LittleEndian>(self.size)?;
        buffer.write_all(&self.blob)?;
        Ok(())
    }

    fn get_mac(&self) -> &[u8] {
        &self.mac
    }

    fn set_mac(&mut self, mac: &[u8]) {
        self.mac.clone_from_slice(mac);
    }
}

impl SrdDelegate {
    pub fn new(b: &[u8], integrity_key: &[u8], delegation_key: &[u8], iv: &[u8]) -> Result<Self> {
        let blob = encrypt_data(b, delegation_key, iv)?;
        let mut response = SrdDelegate {
            signature: SRD_SIGNATURE,
            packet_type: SRD_DELEGATE_MSG_ID,
            seq_num: 4,
            flags: SRD_FLAG_MAC,
            size: (blob.len() as u32),
            blob,
            mac: [0u8; 32],
        };

        response.compute_mac(&integrity_key)?;
        Ok(response)
    }

    pub fn get_data(&self, key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
        Ok(decrypt_data(&self.blob, key, iv)?)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn encrypt_data(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    let mut enc_data = Vec::new();
    {
        let mut cipher = aes::cbc_encryptor(aes::KeySize::KeySize256, key, iv, NoPadding);
        let mut read_buffer = buffer::RefReadBuffer::new(data);
        let mut write_buffer = buffer::RefWriteBuffer::new(&mut enc_data);

        cipher.encrypt(&mut read_buffer, &mut write_buffer, false)?;
    }
    Ok(enc_data)
}

#[cfg(all(target_arch = "wasm32"))]
fn encrypt_data(username: &[u8], password: &[u8], key: &[u8], iv: &[u8]) -> Result<()> {
    //  The library is really barebone, so we need to reimplement CBC
    let cipher = Aes256::new_varkey(key)?;

    let mut data = Vec::new();
    data.write_all(username)?;
    data.write_all(password)?;

    let mut result = Vec::with_capacity(256 + 16);

    // First block is IV
    result.extend_from_slice(&iv[0..16]);

    for i in 0..16 {
        let mut b = GenericArray::clone_from_slice(&xor_block(
            &result[i * 16..i * 16 + 16],
            &data[i * 16..i * 16 + 16],
        ));
        cipher.encrypt_block(&mut b);
        result.extend_from_slice(b.as_slice());
    }

    self.data.clone_from_slice(&result[16..256 + 16]);

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn decrypt_data(enc_data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    {
        let mut cipher = aes::cbc_decryptor(aes::KeySize::KeySize256, key, iv, NoPadding);
        let mut read_buffer = buffer::RefReadBuffer::new(enc_data);
        let mut write_buffer = buffer::RefWriteBuffer::new(&mut data);

        cipher.decrypt(&mut read_buffer, &mut write_buffer, true)?;
    }
    Ok(data)
}

#[cfg(all(target_arch = "wasm32"))]
pub fn decrypt_data(key: &[u8], iv: &[u8]) -> Result<[u8; 256]> {
    let cipher = Aes256::new_varkey(key)?;

    let mut result = Vec::with_capacity(256);

    let b = GenericArray::clone_from_slice(&[0u8; 16]);
    let mut blocks1 = GenericArray::clone_from_slice(&[b; 8]);
    let mut blocks2 = GenericArray::clone_from_slice(&[b; 8]);

    for i in 0..8 {
        blocks1[i] = GenericArray::clone_from_slice(&self.data[i * 16..i * 16 + 16]);
    }
    for i in 8..16 {
        blocks2[i - 8] = GenericArray::clone_from_slice(&self.data[i * 16..i * 16 + 16]);
    }

    cipher.decrypt_blocks(&mut blocks1);
    cipher.decrypt_blocks(&mut blocks2);

    result.extend_from_slice(&xor_block(&iv[0..16], blocks1[0].as_slice()));

    for i in 1..8 {
        result.extend_from_slice(&xor_block(
            &self.data[(i - 1) * 16..(i - 1) * 16 + 16],
            blocks1[i].as_slice(),
        ));
    }
    for i in 8..16 {
        result.extend_from_slice(&xor_block(
            &self.data[(i - 1) * 16..(i - 1) * 16 + 16],
            blocks2[i - 8].as_slice(),
        ));
    }

    let mut data = [0u8; 256];
    data.clone_from_slice(&result);

    println!("{:?}", data.to_vec());

    Ok(data)
}

#[cfg(all(target_arch = "wasm32"))]
fn xor_block(a: &[u8], b: &[u8]) -> [u8; 16] {
    let mut result = [0u8; 16];
    for i in 0..16 {
        result[i] = a[i] ^ b[i];
    }

    result
}