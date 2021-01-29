// https://docs.aws.amazon.com/amazonglacier/latest/dev/checksum-calculations.html#checksum-calculations-upload-archive-in-single-payload
// Adapted from : https://github.com/joechrz/treehash

use sha2::{Sha256, Digest};
use std::fs::File;
use std::io::Read;
use std::io;

/****************************************************************
 * Constants and Types
 ****************************************************************/

const ONE_MB: usize = 1048576;

struct TreeHashStackFrame {
    level: u64,
    bytes: Vec<u8>
}

/****************************************************************
 * Helper functions
 ****************************************************************/

pub fn run_sha256(bytes: &[u8]) -> Vec<u8> {
    let mut sha256 = Sha256::new();
    sha256.update(&bytes);
    let result = sha256.finalize();

    result.iter().copied().collect()
}

pub fn to_hex_string(bytes: &[u8]) -> String {
    let hex_str = String::with_capacity(64);
    bytes.iter()
        .map(|b| format!("{:02x}", b))
        .fold(hex_str, |mut str_agg, item| {
            str_agg.push_str(&item);
            str_agg
        })
}

/****************************************************************
 * Main Implementation
 ****************************************************************/

fn rollup(lbytes: &[u8], rbytes: &[u8]) -> Vec<u8> {

    let mut merge_buf: [u8; 64] = [0; 64];

    merge_buf[..32].clone_from_slice(&lbytes[..32]);
    merge_buf[32..(32 + 32)].clone_from_slice(&rbytes[..32]);

    run_sha256(&merge_buf)
}

/* collapse_stack makes sure the you'll need at most [ceil(log2(file_size_in_mb)) + 1] stack frames
 * (1 per level + a buffer frame) to compute the tree hash of the total file.
 *
 * while (stack has multiple frames)
 *   pop 2 frames and attempt to combine them.
 *   if (2 frames are not combined)
 *     stop iterating.
 *
 * 2 frames are combined when they're at the same level or the 'force' flag is true
 */
fn collapse_stack(stack: &mut Vec<TreeHashStackFrame>, force: bool) {
    loop {
        if stack.len() < 2 {
            return;
        }

        // short-circuit guarantees at least length 2, so unwrap() is ok
        let right = stack.pop().unwrap();
        let left = stack.pop().unwrap();

        if left.level == right.level || force {
            let rolled_up = rollup(&left.bytes, &right.bytes);

            stack.push(TreeHashStackFrame {
                level: left.level + 1,
                bytes: rolled_up
            });
        } else {
            stack.push(left);
            stack.push(right);
            return;
        }
    }
}

pub fn tree_hash(
    // file : &mut File
    filename: &str
) -> Result<Vec<u8>, anyhow::Error> {

    // 32 should handle pretty large (several gb) files without reallocating
    let mut stack: Vec<TreeHashStackFrame> = Vec::with_capacity(32);
    let mut buf: [u8; ONE_MB] = [0; ONE_MB];
    let mut read_from: Box<dyn io::Read> = Box::new(
        // file
        File::open(filename)?
    );

    loop {

        let bytes_read = read_from.read(&mut buf).unwrap();
        if bytes_read == 0 {
            break;
        }

        // read a <= 1MB chunk, compute the sha256, and push onto the stack
        let data_slice = &buf[0..bytes_read];

        stack.push(TreeHashStackFrame {
            level: 0,
            bytes: run_sha256(&data_slice)
        });

        // then optimize the stack (collapse like-levels into a higher level)
        collapse_stack(&mut stack, false);
    }

    // force-combine the last bits (eg: promote frames that don't have a pair at their own level)
    collapse_stack(&mut stack, true);

    // the last frame contains the entire file's hash
    match stack.pop() {
        Some(final_frame) => Ok(final_frame.bytes),
        None => panic!("Something went horribly wrong")
    }
}
