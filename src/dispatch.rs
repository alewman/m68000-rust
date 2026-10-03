//! The opcode map: which handler each of the 65,536 first words runs.
//!
//! Transcribed from `m68000_python/_dispatch.py`. [`RULES`] is the 68000
//! instruction set as PRM Section 8 prints it, one line per encoding: a
//! 16-character bit pattern, the handler, and the operand constraint. In a
//! pattern `0` and `1` are fixed bits and letters are fields: `ss` a size
//! (00 byte, 01 word, 10 long; 11 is never that instruction), `eeeeee` an
//! effective address, anything else a field the handler decodes. The
//! constraint names the addressing category of PRM Table 2-4 the EA field
//! must fall in; an EA outside it makes the word something else or illegal
//! (UM 6.3.6).
//!
//! [`names`] walks all 65,536 words once, at first use, and checks that no
//! word matches two rules. A word that matches none runs the
//! illegal-instruction exception, or the line 1010 / line 1111 emulator
//! exception when its top four bits say so (UM 6.3.6). Dispatch is then one
//! table index and one `match` per instruction.
//!
//! The reference's table holds bound handler functions, built per class;
//! here the table holds the handler's name as an [`Op`] value (the
//! reference's `NAMES`), and [`M68000::execute`] is the `match` from name to
//! handler.

use std::sync::OnceLock;

use crate::core::{Bus, Step, M68000};
use crate::ea::{
    ea_kind, within, Category, ALL, ALTERABLE, AN, CONTROL, DATA, DATA_ALTERABLE, EA_KIND,
    MEMORY_ALTERABLE, NONE, POSTINC, PREDEC,
};

/// Every handler, by the name the reference gives it (`_op_<name>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(non_camel_case_types)]
pub enum Op {
    ori_to_ccr,
    ori_to_sr,
    ori,
    andi_to_ccr,
    andi_to_sr,
    andi,
    subi,
    addi,
    eori_to_ccr,
    eori_to_sr,
    eori,
    cmpi,
    btst,
    bchg,
    bclr,
    bset,
    movep,
    r#move,
    movea,
    move_from_sr,
    negx,
    clr,
    move_to_ccr,
    neg,
    move_to_sr,
    not,
    nbcd,
    swap,
    pea,
    ext,
    movem,
    illegal,
    tas,
    tst,
    trap,
    link,
    unlk,
    move_usp,
    reset,
    nop,
    stop,
    rte,
    rts,
    trapv,
    rtr,
    jsr,
    jmp,
    chk,
    lea,
    addq,
    subq,
    dbcc,
    scc,
    bra,
    bsr,
    bcc,
    moveq,
    divu,
    divs,
    sbcd,
    or,
    suba,
    subx,
    sub,
    cmpa,
    cmpm,
    cmp,
    eor,
    mulu,
    muls,
    abcd,
    exg,
    and,
    adda,
    addx,
    add,
    asd_memory,
    lsd_memory,
    roxd_memory,
    rod_memory,
    asd,
    lsd,
    roxd,
    rod,
    line_a,
    line_f,
}

/// An operand constraint: does `opcode` satisfy the rule's EA restriction?
#[derive(Clone, Copy)]
pub enum Check {
    /// The EA in bits 5-0 must be in the category.
    Ea(Category),
    /// Size in bits 7-6 is not 11, the EA is in the category, and An is never a byte.
    Sized(Category),
    /// Register forms with a size field (ADDX, SUBX, CMPM, shifts): not 11.
    AnySizeRegister,
    /// MOVE: source any mode (no An for a byte), destination data alterable.
    Move,
    /// MOVEA: any source mode, word or long only.
    Movea,
    /// Bcc: the condition is not T or F (those are BRA and BSR).
    Bcc,
    Always,
}

impl Check {
    fn holds(self, opcode: u16) -> bool {
        match self {
            Check::Ea(category) => within(ea_kind(opcode), category),
            Check::Sized(category) => {
                let size_bits = (opcode >> 6) & 3;
                let kind = ea_kind(opcode);
                if size_bits == 3 || !within(kind, category) {
                    return false;
                }
                !(size_bits == 0 && kind == AN)
            }
            Check::AnySizeRegister => (opcode >> 6) & 3 != 3,
            Check::Move => {
                let source = ea_kind(opcode);
                let destination =
                    EA_KIND[usize::from(((opcode >> 3) & 0x38) | ((opcode >> 9) & 7))];
                if source == NONE || !within(destination, DATA_ALTERABLE) {
                    return false;
                }
                !((opcode >> 12) == 1 && source == AN)
            }
            Check::Movea => ea_kind(opcode) != NONE,
            Check::Bcc => ((opcode >> 8) & 0xF) >= 2,
            Check::Always => true,
        }
    }
}

const CONTROL_ALTERABLE: Category = CONTROL & ALTERABLE;
const fn bit(kind: u8) -> Category {
    1 << kind
}
/// `DATA - {11}`: BTST #n,<ea> takes no immediate operand.
const DATA_NOT_IMMEDIATE: Category = DATA & !bit(11);

use Check::{Always, AnySizeRegister, Ea, Sized};

/// (bit pattern, handler, constraint) -- PRM Section 8, in opcode-map order.
pub const RULES: &[(&str, Op, Check)] = &[
    // 0000: bit manipulation, MOVEP, immediate
    ("0000 0000 0011 1100", Op::ori_to_ccr, Always),
    ("0000 0000 0111 1100", Op::ori_to_sr, Always),
    ("0000 0000 ssee eeee", Op::ori, Sized(DATA_ALTERABLE)),
    ("0000 0010 0011 1100", Op::andi_to_ccr, Always),
    ("0000 0010 0111 1100", Op::andi_to_sr, Always),
    ("0000 0010 ssee eeee", Op::andi, Sized(DATA_ALTERABLE)),
    ("0000 0100 ssee eeee", Op::subi, Sized(DATA_ALTERABLE)),
    ("0000 0110 ssee eeee", Op::addi, Sized(DATA_ALTERABLE)),
    ("0000 1010 0011 1100", Op::eori_to_ccr, Always),
    ("0000 1010 0111 1100", Op::eori_to_sr, Always),
    ("0000 1010 ssee eeee", Op::eori, Sized(DATA_ALTERABLE)),
    ("0000 1100 ssee eeee", Op::cmpi, Sized(DATA_ALTERABLE)),
    ("0000 1000 00ee eeee", Op::btst, Ea(DATA_NOT_IMMEDIATE)),
    ("0000 1000 01ee eeee", Op::bchg, Ea(DATA_ALTERABLE)),
    ("0000 1000 10ee eeee", Op::bclr, Ea(DATA_ALTERABLE)),
    ("0000 1000 11ee eeee", Op::bset, Ea(DATA_ALTERABLE)),
    ("0000 rrr1 00ee eeee", Op::btst, Ea(DATA)),
    ("0000 rrr1 01ee eeee", Op::bchg, Ea(DATA_ALTERABLE)),
    ("0000 rrr1 10ee eeee", Op::bclr, Ea(DATA_ALTERABLE)),
    ("0000 rrr1 11ee eeee", Op::bset, Ea(DATA_ALTERABLE)),
    ("0000 rrr1 oo00 1aaa", Op::movep, Always),
    // 0001, 0010, 0011: MOVE and MOVEA
    ("0001 RRRM MMee eeee", Op::r#move, Check::Move),
    ("0011 RRRM MMee eeee", Op::r#move, Check::Move),
    ("0010 RRRM MMee eeee", Op::r#move, Check::Move),
    ("0011 aaa0 01ee eeee", Op::movea, Check::Movea),
    ("0010 aaa0 01ee eeee", Op::movea, Check::Movea),
    // 0100: miscellaneous
    ("0100 0000 11ee eeee", Op::move_from_sr, Ea(DATA_ALTERABLE)),
    ("0100 0000 ssee eeee", Op::negx, Sized(DATA_ALTERABLE)),
    ("0100 0010 ssee eeee", Op::clr, Sized(DATA_ALTERABLE)),
    ("0100 0100 11ee eeee", Op::move_to_ccr, Ea(DATA)),
    ("0100 0100 ssee eeee", Op::neg, Sized(DATA_ALTERABLE)),
    ("0100 0110 11ee eeee", Op::move_to_sr, Ea(DATA)),
    ("0100 0110 ssee eeee", Op::not, Sized(DATA_ALTERABLE)),
    ("0100 1000 00ee eeee", Op::nbcd, Ea(DATA_ALTERABLE)),
    ("0100 1000 0100 0rrr", Op::swap, Always),
    ("0100 1000 01ee eeee", Op::pea, Ea(CONTROL)),
    ("0100 1000 1000 0rrr", Op::ext, Always),
    ("0100 1000 1100 0rrr", Op::ext, Always),
    (
        "0100 1000 1see eeee",
        Op::movem,
        Ea(CONTROL_ALTERABLE | bit(PREDEC)),
    ),
    ("0100 1010 1111 1100", Op::illegal, Always),
    ("0100 1010 11ee eeee", Op::tas, Ea(DATA_ALTERABLE)),
    ("0100 1010 ssee eeee", Op::tst, Sized(DATA_ALTERABLE)),
    ("0100 1100 1see eeee", Op::movem, Ea(CONTROL | bit(POSTINC))),
    ("0100 1110 0100 vvvv", Op::trap, Always),
    ("0100 1110 0101 0rrr", Op::link, Always),
    ("0100 1110 0101 1rrr", Op::unlk, Always),
    ("0100 1110 0110 drrr", Op::move_usp, Always),
    ("0100 1110 0111 0000", Op::reset, Always),
    ("0100 1110 0111 0001", Op::nop, Always),
    ("0100 1110 0111 0010", Op::stop, Always),
    ("0100 1110 0111 0011", Op::rte, Always),
    ("0100 1110 0111 0101", Op::rts, Always),
    ("0100 1110 0111 0110", Op::trapv, Always),
    ("0100 1110 0111 0111", Op::rtr, Always),
    ("0100 1110 10ee eeee", Op::jsr, Ea(CONTROL)),
    ("0100 1110 11ee eeee", Op::jmp, Ea(CONTROL)),
    ("0100 rrr1 10ee eeee", Op::chk, Ea(DATA)),
    ("0100 rrr1 11ee eeee", Op::lea, Ea(CONTROL)),
    // 0101: ADDQ, SUBQ, Scc, DBcc
    ("0101 qqq0 ssee eeee", Op::addq, Sized(ALTERABLE)),
    ("0101 qqq1 ssee eeee", Op::subq, Sized(ALTERABLE)),
    ("0101 cccc 1100 1rrr", Op::dbcc, Always),
    ("0101 cccc 11ee eeee", Op::scc, Ea(DATA_ALTERABLE)),
    // 0110: Bcc, BRA, BSR; 0111: MOVEQ
    ("0110 0000 dddd dddd", Op::bra, Always),
    ("0110 0001 dddd dddd", Op::bsr, Always),
    ("0110 cccc dddd dddd", Op::bcc, Check::Bcc),
    ("0111 rrr0 dddd dddd", Op::moveq, Always),
    // 1000: OR, DIVU, DIVS, SBCD
    ("1000 rrr0 11ee eeee", Op::divu, Ea(DATA)),
    ("1000 rrr1 11ee eeee", Op::divs, Ea(DATA)),
    ("1000 yyy1 0000 mxxx", Op::sbcd, Always),
    ("1000 rrr0 ssee eeee", Op::or, Sized(DATA)),
    ("1000 rrr1 ssee eeee", Op::or, Sized(MEMORY_ALTERABLE)),
    // 1001: SUB, SUBA, SUBX
    ("1001 aaas 11ee eeee", Op::suba, Ea(ALL)),
    ("1001 yyy1 ss00 mxxx", Op::subx, AnySizeRegister),
    ("1001 rrr0 ssee eeee", Op::sub, Sized(ALL)),
    ("1001 rrr1 ssee eeee", Op::sub, Sized(MEMORY_ALTERABLE)),
    // 1011: CMP, CMPA, CMPM, EOR
    ("1011 aaas 11ee eeee", Op::cmpa, Ea(ALL)),
    ("1011 yyy1 ss00 1xxx", Op::cmpm, AnySizeRegister),
    ("1011 rrr0 ssee eeee", Op::cmp, Sized(ALL)),
    ("1011 rrr1 ssee eeee", Op::eor, Sized(DATA_ALTERABLE)),
    // 1100: AND, MULU, MULS, ABCD, EXG
    ("1100 rrr0 11ee eeee", Op::mulu, Ea(DATA)),
    ("1100 rrr1 11ee eeee", Op::muls, Ea(DATA)),
    ("1100 yyy1 0000 mxxx", Op::abcd, Always),
    ("1100 xxx1 0100 0yyy", Op::exg, Always),
    ("1100 xxx1 0100 1yyy", Op::exg, Always),
    ("1100 xxx1 1000 1yyy", Op::exg, Always),
    ("1100 rrr0 ssee eeee", Op::and, Sized(DATA)),
    ("1100 rrr1 ssee eeee", Op::and, Sized(MEMORY_ALTERABLE)),
    // 1101: ADD, ADDA, ADDX
    ("1101 aaas 11ee eeee", Op::adda, Ea(ALL)),
    ("1101 yyy1 ss00 mxxx", Op::addx, AnySizeRegister),
    ("1101 rrr0 ssee eeee", Op::add, Sized(ALL)),
    ("1101 rrr1 ssee eeee", Op::add, Sized(MEMORY_ALTERABLE)),
    // 1110: shifts and rotates, memory (word, by one) then register forms
    ("1110 000d 11ee eeee", Op::asd_memory, Ea(MEMORY_ALTERABLE)),
    ("1110 001d 11ee eeee", Op::lsd_memory, Ea(MEMORY_ALTERABLE)),
    ("1110 010d 11ee eeee", Op::roxd_memory, Ea(MEMORY_ALTERABLE)),
    ("1110 011d 11ee eeee", Op::rod_memory, Ea(MEMORY_ALTERABLE)),
    ("1110 cccd ssi0 0rrr", Op::asd, AnySizeRegister),
    ("1110 cccd ssi0 1rrr", Op::lsd, AnySizeRegister),
    ("1110 cccd ssi1 0rrr", Op::roxd, AnySizeRegister),
    ("1110 cccd ssi1 1rrr", Op::rod, AnySizeRegister),
];

fn mask_and_value(pattern: &str) -> (u16, u16) {
    let bits: Vec<char> = pattern.chars().filter(|c| *c != ' ').collect();
    assert_eq!(bits.len(), 16, "{pattern}");
    let (mut mask, mut value) = (0u16, 0u16);
    for bit in bits {
        mask <<= 1;
        value <<= 1;
        if bit == '0' || bit == '1' {
            mask |= 1;
            value |= u16::from(bit == '1');
        }
    }
    (mask, value)
}

/// The handler name for `opcode`: a rule's, or illegal / line_a / line_f.
pub fn decode(opcode: u16) -> Op {
    let mut found = None;
    for (pattern, name, check) in RULES {
        let (mask, value) = mask_and_value(pattern);
        if opcode & mask == value && check.holds(opcode) {
            assert!(
                found.is_none(),
                "{opcode:04X} matches {found:?} and {name:?}"
            );
            found = Some(*name);
        }
    }
    if let Some(name) = found {
        return name;
    }
    match opcode >> 12 {
        0xA => Op::line_a,
        0xF => Op::line_f,
        _ => Op::illegal,
    }
}

/// The handler name of every first word, computed once, at first use.
pub fn names() -> &'static [Op; 0x10000] {
    static NAMES: OnceLock<Box<[Op; 0x10000]>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let compiled: Vec<(u16, u16, Op, Check)> = RULES
            .iter()
            .map(|(pattern, name, check)| {
                let (mask, value) = mask_and_value(pattern);
                (mask, value, *name, *check)
            })
            .collect();
        let mut table = Box::new([Op::illegal; 0x10000]);
        for opcode in 0..=0xFFFFu16 {
            let mut found = None;
            for &(mask, value, name, check) in &compiled {
                if opcode & mask == value && check.holds(opcode) {
                    assert!(
                        found.is_none(),
                        "{opcode:04X} matches {found:?} and {name:?}"
                    );
                    found = Some(name);
                }
            }
            table[usize::from(opcode)] = found.unwrap_or(match opcode >> 12 {
                0xA => Op::line_a,
                0xF => Op::line_f,
                _ => Op::illegal,
            });
        }
        table
    })
}

impl<B: Bus> M68000<B> {
    /// Run the handler named `op` for `opcode`: the reference's `table[opcode](cpu, opcode)`.
    pub(crate) fn execute(&mut self, op: Op, opcode: u16) -> Step {
        match op {
            Op::ori_to_ccr => self.op_ori_to_ccr(opcode),
            Op::ori_to_sr => self.op_ori_to_sr(opcode),
            Op::ori => self.op_ori(opcode),
            Op::andi_to_ccr => self.op_andi_to_ccr(opcode),
            Op::andi_to_sr => self.op_andi_to_sr(opcode),
            Op::andi => self.op_andi(opcode),
            Op::subi => self.op_subi(opcode),
            Op::addi => self.op_addi(opcode),
            Op::eori_to_ccr => self.op_eori_to_ccr(opcode),
            Op::eori_to_sr => self.op_eori_to_sr(opcode),
            Op::eori => self.op_eori(opcode),
            Op::cmpi => self.op_cmpi(opcode),
            Op::btst => self.op_btst(opcode),
            Op::bchg => self.op_bchg(opcode),
            Op::bclr => self.op_bclr(opcode),
            Op::bset => self.op_bset(opcode),
            Op::movep => self.op_movep(opcode),
            Op::r#move => self.op_move(opcode),
            Op::movea => self.op_movea(opcode),
            Op::move_from_sr => self.op_move_from_sr(opcode),
            Op::negx => self.op_negx(opcode),
            Op::clr => self.op_clr(opcode),
            Op::move_to_ccr => self.op_move_to_ccr(opcode),
            Op::neg => self.op_neg(opcode),
            Op::move_to_sr => self.op_move_to_sr(opcode),
            Op::not => self.op_not(opcode),
            Op::nbcd => self.op_nbcd(opcode),
            Op::swap => self.op_swap(opcode),
            Op::pea => self.op_pea(opcode),
            Op::ext => self.op_ext(opcode),
            Op::movem => self.op_movem(opcode),
            Op::illegal => self.op_illegal(opcode),
            Op::tas => self.op_tas(opcode),
            Op::tst => self.op_tst(opcode),
            Op::trap => self.op_trap(opcode),
            Op::link => self.op_link(opcode),
            Op::unlk => self.op_unlk(opcode),
            Op::move_usp => self.op_move_usp(opcode),
            Op::reset => self.op_reset(opcode),
            Op::nop => self.op_nop(opcode),
            Op::stop => self.op_stop(opcode),
            Op::rte => self.op_rte(opcode),
            Op::rts => self.op_rts(opcode),
            Op::trapv => self.op_trapv(opcode),
            Op::rtr => self.op_rtr(opcode),
            Op::jsr => self.op_jsr(opcode),
            Op::jmp => self.op_jmp(opcode),
            Op::chk => self.op_chk(opcode),
            Op::lea => self.op_lea(opcode),
            Op::addq => self.op_addq(opcode),
            Op::subq => self.op_subq(opcode),
            Op::dbcc => self.op_dbcc(opcode),
            Op::scc => self.op_scc(opcode),
            Op::bra => self.op_bra(opcode),
            Op::bsr => self.op_bsr(opcode),
            Op::bcc => self.op_bcc(opcode),
            Op::moveq => self.op_moveq(opcode),
            Op::divu => self.op_divu(opcode),
            Op::divs => self.op_divs(opcode),
            Op::sbcd => self.op_sbcd(opcode),
            Op::or => self.op_or(opcode),
            Op::suba => self.op_suba(opcode),
            Op::subx => self.op_subx(opcode),
            Op::sub => self.op_sub(opcode),
            Op::cmpa => self.op_cmpa(opcode),
            Op::cmpm => self.op_cmpm(opcode),
            Op::cmp => self.op_cmp(opcode),
            Op::eor => self.op_eor(opcode),
            Op::mulu => self.op_mulu(opcode),
            Op::muls => self.op_muls(opcode),
            Op::abcd => self.op_abcd(opcode),
            Op::exg => self.op_exg(opcode),
            Op::and => self.op_and(opcode),
            Op::adda => self.op_adda(opcode),
            Op::addx => self.op_addx(opcode),
            Op::add => self.op_add(opcode),
            Op::asd_memory => self.op_asd_memory(opcode),
            Op::lsd_memory => self.op_lsd_memory(opcode),
            Op::roxd_memory => self.op_roxd_memory(opcode),
            Op::rod_memory => self.op_rod_memory(opcode),
            Op::asd => self.op_asd(opcode),
            Op::lsd => self.op_lsd(opcode),
            Op::roxd => self.op_roxd(opcode),
            Op::rod => self.op_rod(opcode),
            Op::line_a => self.op_line_a(opcode),
            Op::line_f => self.op_line_f(opcode),
        }
    }
}
