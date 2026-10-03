//! Program control: branches, jumps, returns, DBcc, Scc, NOP (PRM Table 3-9).
//!
//! Transcribed from `m68000_python/_control.py`. A taken branch or jump
//! throws the prefetch queue away and refills it from the target with two
//! program reads (UM Table 8-9). A target at an odd address faults on the
//! first of those reads.

use crate::core::{Bus, Step, M68000};
use crate::ea::{
    ea_kind, sign_extend_16, sign_extend_8, word_to_long, ABSL, ABSW, DISP, DN, IND, INDEX, PCDISP,
    PCINDEX,
};
use crate::flags::condition;

impl<B: Bus> M68000<B> {
    // -- branches (PRM 4, Bcc/BRA/BSR; UM Table 8-9) -------------------------

    /// The target of a Bcc/BRA/BSR: displacement from the extension word's address.
    ///
    /// An 8-bit displacement of 0 means a 16-bit one follows; $FF is an
    /// ordinary -1 on the 68000 (the 68020's long form does not exist;
    /// corpus issue #4).
    fn branch_target(&self, opcode: u16) -> u32 {
        let base = self.pc.wrapping_sub(2); // the address of the word after the opcode
        let displacement = u32::from(opcode & 0xFF);
        if displacement != 0 {
            return base.wrapping_add(sign_extend_8(displacement) as u32);
        }
        base.wrapping_add(sign_extend_16(u32::from(self.irc)) as u32)
    }

    /// BRA -- branch always: PC <- PC + d (PRM 4-25; UM Table 8-9: 10 clocks).
    pub(crate) fn op_bra(&mut self, opcode: u16) -> Step {
        self.cycles += 2;
        self.jump(self.branch_target(opcode))
    }

    /// Bcc -- branch if the condition holds (PRM 4-25; UM Table 8-9).
    ///
    /// Taken: 10 clocks, two internal then the refill. Not taken with an
    /// 8-bit displacement: 8 clocks; with a 16-bit one, 12, the extra read
    /// skipping the displacement word.
    pub(crate) fn op_bcc(&mut self, opcode: u16) -> Step {
        if condition(opcode, self.sr) {
            self.cycles += 2;
            return self.jump(self.branch_target(opcode));
        }
        self.cycles += 4;
        if opcode & 0xFF == 0 {
            self.extension()?;
        }
        self.prefetch()
    }

    /// BSR -- push the return address, then branch (PRM 4-59; UM Table 8-9: 18 clocks).
    ///
    /// A fault on the refill stacks the target itself: SST BSR.
    pub(crate) fn op_bsr(&mut self, opcode: u16) -> Step {
        let target = self.branch_target(opcode);
        // The return address is past the displacement word, when there is one.
        let returns = if opcode & 0xFF == 0 {
            self.pc
        } else {
            self.pc.wrapping_sub(2)
        };
        self.cycles += 2;
        self.push_long(returns)?;
        self.fault_pc = target; // a fault on the refill stacks the target
        self.jump(target)
    }

    // -- DBcc and Scc (PRM 4-91, 4-173) ---------------------------------------

    /// DBcc -- if cc is false, decrement Dn.w; branch unless it is -1 (PRM 4-91; UM Table 8-9).
    ///
    /// UM Table 8-9: condition true 12 clocks; false and branching 10; false
    /// with the count expired 14.
    ///
    /// The branch target's word is read before Dn is written, so an odd
    /// target faults with Dn unchanged and stacks the instruction's address
    /// + 4: SST DBcc. WinUAE run stacks the target + 2 instead: contested,
    /// the core follows the gate.
    pub(crate) fn op_dbcc(&mut self, opcode: u16) -> Step {
        let register = usize::from(opcode & 7);
        if condition(opcode, self.sr) {
            self.cycles += 4;
            self.extension()?;
            return self.prefetch();
        }
        // Condition false: the word at the branch target is read whether or
        // not the count expires, and Dn is written only after that read, so
        // an odd target faults with Dn unchanged.
        self.cycles += 2;
        let target = self
            .pc
            .wrapping_sub(2)
            .wrapping_add(sign_extend_16(u32::from(self.irc)) as u32);
        self.commit_pc();
        let count = self.r[register].wrapping_sub(1) & 0xFFFF;
        self.irc = self.read_program_word(target)? as u16;
        if count != 0xFFFF {
            self.pc = target.wrapping_add(2);
        } else {
            self.extension()?; // expired: carry on after the displacement word
        }
        self.r[register] = (self.r[register] & 0xFFFF_0000) | count;
        self.prefetch()
    }

    /// Scc -- set a byte to $FF if the condition holds, else $00 (PRM 4-173; UM Table 8-6).
    ///
    /// UM Table 8-6: register 4 clocks false, 6 true; memory 8 plus the EA,
    /// the operand read before it is written. The read, the refill between
    /// it and the write, and the PC an address error stacks: SST Scc.
    pub(crate) fn op_scc(&mut self, opcode: u16) -> Step {
        let value = if condition(opcode, self.sr) { 0xFF } else { 0 };
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        if kind == DN {
            self.prefetch()?;
            if value != 0 {
                self.cycles += 2;
            }
            self.r[register] = (self.r[register] & 0xFFFF_FF00) | value;
            return Ok(());
        }
        let address = self.ea_address(kind, register, 1)?;
        self.read_byte(address)?;
        self.prefetch()?;
        self.write_byte(address, value)
    }

    // -- jumps and returns ----------------------------------------------------

    /// The address of a control-mode operand, for JMP/JSR/LEA/PEA (UM Table 8-10).
    ///
    /// Unlike an operand read, these modes do not take the extension word
    /// from the queue as they go: the refill happens later, as part of the
    /// jump or the next step. Internal clocks are the table's.
    fn control_address(
        &mut self,
        kind: u8,
        register: usize,
    ) -> Result<u32, crate::core::GroupZero> {
        let irc = u32::from(self.irc);
        Ok(match kind {
            IND => self.r[8 + register],
            DISP => self.r[8 + register].wrapping_add(sign_extend_16(irc) as u32),
            INDEX => self.indexed(self.r[8 + register], irc),
            ABSW => word_to_long(irc),
            ABSL => {
                let high = self.extension()?;
                (high << 16) | u32::from(self.irc)
            }
            PCDISP => self
                .pc
                .wrapping_sub(2)
                .wrapping_add(sign_extend_16(irc) as u32),
            PCINDEX => self.indexed(self.pc.wrapping_sub(2), irc),
            _ => unreachable!("not a control kind: {kind}"),
        })
    }

    /// JMP -- PC <- effective address (PRM 4-108; UM Table 8-10).
    pub(crate) fn op_jmp(&mut self, opcode: u16) -> Step {
        let kind = ea_kind(opcode);
        let target = self.control_address(kind, usize::from(opcode & 7))?;
        if matches!(kind, DISP | ABSW | PCDISP) {
            self.cycles += 2;
        } else if matches!(kind, INDEX | PCINDEX) {
            self.cycles += 6;
        }
        self.jump(target)
    }

    /// JSR -- push the return address, PC <- effective address (PRM 4-109; UM Table 8-10).
    ///
    /// The target's first word is read before the push, so an odd target
    /// faults with nothing pushed: SST JSR. Through (d16,An), (d8,An,Xn),
    /// (d16,PC) and (d8,PC,Xn) WinUAE run stacks 2 less than the gate:
    /// contested, the core follows the gate.
    pub(crate) fn op_jsr(&mut self, opcode: u16) -> Step {
        let kind = ea_kind(opcode);
        let target = self.control_address(kind, usize::from(opcode & 7))?;
        // The return address is past the extension words.
        let returns = if kind == IND {
            self.pc.wrapping_sub(2)
        } else {
            self.pc
        };
        if matches!(kind, DISP | ABSW | PCDISP) {
            self.cycles += 2;
        } else if matches!(kind, INDEX | PCINDEX) {
            self.cycles += 6;
        }
        if kind != IND {
            self.commit_pc();
        }
        // The target's first word is read before the push, so an odd target
        // faults with nothing pushed.
        self.irc = self.read_program_word(target)? as u16;
        self.pc = target.wrapping_add(2);
        self.push_long(returns)?;
        self.prefetch()
    }

    /// RTS -- PC <- (SP)+ (PRM 4-169; UM Table 8-12: 16 clocks).
    pub(crate) fn op_rts(&mut self, _opcode: u16) -> Step {
        let target = self.pop_long()?;
        self.jump(target)
    }

    /// RTR -- CCR <- (SP)+, then PC <- (SP)+ (PRM 4-168; UM Table 8-12: 20 clocks).
    pub(crate) fn op_rtr(&mut self, _opcode: u16) -> Step {
        let ccr = self.pop_word()?;
        self.set_ccr(ccr as u16);
        let target = self.pop_long()?;
        self.jump(target)
    }

    /// NOP -- no operation (PRM 4-147; UM Table 8-12: 4 clocks, the prefetch).
    pub(crate) fn op_nop(&mut self, _opcode: u16) -> Step {
        self.prefetch()
    }
}
