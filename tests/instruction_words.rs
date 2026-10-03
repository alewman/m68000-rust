//! The trace producer's instruction lengths agree with the reference's disassembler.
//!
//! `tests/data/instruction-words.bin` holds, for every first word, how many
//! words m68000-python's `disassemble` reads at commit 38dbd0f (generated with
//! extension words of zero; no length depends on their values).

use m68000_rust::trace::instruction_words;

#[test]
fn every_first_word_has_the_reference_length() {
    let expected = std::fs::read("tests/data/instruction-words.bin").unwrap();
    assert_eq!(expected.len(), 0x10000);
    for opcode in 0..=0xFFFFu16 {
        assert_eq!(
            instruction_words(opcode),
            usize::from(expected[usize::from(opcode)]),
            "opcode {opcode:04X}"
        );
    }
}
