//! The converter on mutated `PrimJS` bytecode: fixed-seed mutations of
//! corpus sections and fixture samples, read and converted. Each input is
//! refused by the reader or the converter, or converts to bytes that pass
//! the bridge's checks; nothing panics. The outputs are not executed.

#[path = "../../../packages/reactlynx-test-fixtures/bytecode.rs"]
#[allow(
    clippy::doc_markdown,
    reason = "the registry is the fixture package's file"
)]
mod bytecode;

use bobcat_source::native::primjs::{self, ConvertOptions};
use quickjs_rust_bridge::bytecode::Verifier;

fn seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for program in bytecode::PROGRAMS {
        for variant in [bytecode::Variant::Sdk4_1, bytecode::Variant::InlineDebug] {
            for section in primjs::bundle_bytecode_sections(program.bundle(variant)).unwrap() {
                // Packed payloads are unwrapped, so mutations reach the
                // bytecode rather than the packed header.
                let bytes = match primjs::unwrap_packed(section.bytes).unwrap() {
                    Some((_, inner)) => inner.to_vec(),
                    None => section.bytes.to_vec(),
                };
                seeds.push(bytes);
            }
        }
    }
    seeds
}

/// xorshift64*, so a failure reproduces from the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next() % bound as u64).expect("modulus keeps this under `bound`")
    }
}

/// LEB128 encodings of large counts, spliced over a byte to inflate whatever
/// count or index sits there.
const INFLATED: &[&[u8]] = &[
    &[0xff, 0xff, 0xff, 0xff, 0x0f],
    &[0xff, 0xff, 0x03],
    &[0x80, 0x80, 0x80, 0x80, 0x10],
    &[0xff, 0x7f],
    &[0xff],
];

fn mutate(bytes: &mut Vec<u8>, rng: &mut Rng) {
    match rng.below(7) {
        0 if !bytes.is_empty() => {
            let at = rng.below(bytes.len());
            bytes[at] ^= 1 << rng.below(8);
        }
        1 if !bytes.is_empty() => {
            let at = rng.below(bytes.len());
            bytes[at] = (rng.next() & 0xff) as u8;
        }
        2 => bytes.truncate(rng.below(bytes.len() + 1)),
        3 if !bytes.is_empty() => {
            bytes.remove(rng.below(bytes.len()));
        }
        4 => {
            let at = rng.below(bytes.len() + 1);
            bytes.insert(at, (rng.next() & 0xff) as u8);
        }
        5 if !bytes.is_empty() => {
            let at = rng.below(bytes.len());
            let inflated = INFLATED[rng.below(INFLATED.len())];
            bytes.splice(at..=at, inflated.iter().copied());
        }
        _ if !bytes.is_empty() => {
            let start = rng.below(bytes.len());
            let end = start + rng.below((bytes.len() - start).min(16) + 1);
            let run = bytes[start..end].to_vec();
            let at = rng.below(bytes.len() + 1);
            bytes.splice(at..at, run);
        }
        _ => {}
    }
}

const ITERATIONS: usize = 100_000;

#[test]
fn mutated_bytecode_converts_or_is_refused() {
    let verifier = Verifier::new();
    let rechecker = Verifier::new();
    let options = ConvertOptions::script("app:///mutated.js");
    let mut rng = Rng(0x5eed_c0de_cafe_0007);
    let seeds = seeds();
    let mut current = seeds[0].clone();
    let (mut read, mut converted) = (0usize, 0usize);
    for iteration in 0..ITERATIONS {
        if iteration % 4 == 0 {
            current = seeds[rng.below(seeds.len())].clone();
        }
        mutate(&mut current, &mut rng);
        if current.len() > 64 * 1024 {
            current.truncate(4096);
        }
        let Ok(program) = primjs::read_program(&current) else {
            continue;
        };
        read += 1;
        if let Ok(module) = primjs::convert_script(&program, &options, &verifier) {
            converted += 1;
            if let Err(error) = rechecker.module_from_bytes(module.bytes()) {
                panic!("iteration {iteration}: converted bytes fail the checks: {error}");
            }
        }
    }
    eprintln!("{ITERATIONS} mutations: {read} read, {converted} converted");
    // Enough mutated programs read and convert that both branches are
    // exercised.
    assert!(
        read >= 1000 && converted >= 500,
        "{read} read, {converted} converted"
    );
}
