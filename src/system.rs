//! System control: traps, the status register, the stack pointers, STOP, RESET.
//!
//! Transcribed from `m68000_python/_system.py`. PRM Table 3-10 and Section
//! 6; the exception processing is UM 6.2-6.3. The privileged instructions
//! check S first and take the privilege-violation exception (vector 8) in
//! user mode, stacking the address of the instruction itself (UM 6.3.7). An
//! instruction that writes SR refills the prefetch queue afterwards,
//! re-reading the next instruction in the (possibly new) program space (SST
//! MOVEtoSR, ORItoSR, ANDItoSR, EORItoSR; T3).

use crate::core::{
    Bus, Step, CCR_BITS, M68000, N, S, V, VECTOR_CHK, VECTOR_ILLEGAL, VECTOR_LINE_A, VECTOR_LINE_F,
    VECTOR_PRIVILEGE, VECTOR_TRAPV, VECTOR_TRAP_BASE, VECTOR_ZERO_DIVIDE, X, Z,
};
use crate::ea::{ea_kind, sign_extend_16, DN};

impl<B: Bus> M68000<B> {
    // -- exceptions an instruction raises -----------------------------------

    /// The exception for an instruction that is never executed, stacking its address.
    ///
    /// Illegal words, the line 1010 and 1111 words, and privileged
    /// instructions in user mode are not executed, so no trace exception
    /// follows them even when T was set (UM 6.3.8; MAME 0.285's microcode
    /// clears the pending trace in these four states, T3; WinUAE keeps it
    /// only after vectors 5-7 and 32-47, T2). `step()` reads `untraced`.
    fn not_executed(&mut self, vector: u32) -> Step {
        self.exception(vector, self.pc.wrapping_sub(4), 4, None, false)?;
        self.untraced = true;
        Ok(())
    }

    /// Vector 8, stacking the address of the offending instruction (UM 6.3.7).
    fn privilege_violation(&mut self) -> Step {
        self.not_executed(VECTOR_PRIVILEGE)
    }

    /// ILLEGAL -- take the illegal-instruction exception, vector 4 (PRM 4-107; UM 6.3.6).
    ///
    /// Also every first word the 68000 does not define. The stacked PC is
    /// the address of the word itself (SST ILLEGAL_LINEA, ILLEGAL_LINEF, and
    /// WinUAE run for $4AFC itself).
    pub(crate) fn op_illegal(&mut self, _opcode: u16) -> Step {
        self.not_executed(VECTOR_ILLEGAL)
    }

    /// LINE A -- words $Axxx: the line 1010 emulator exception, vector 10 (UM 6.3.6).
    pub(crate) fn op_line_a(&mut self, _opcode: u16) -> Step {
        self.not_executed(VECTOR_LINE_A)
    }

    /// LINE F -- words $Fxxx: the line 1111 emulator exception, vector 11 (UM 6.3.6).
    pub(crate) fn op_line_f(&mut self, _opcode: u16) -> Step {
        self.not_executed(VECTOR_LINE_F)
    }

    /// TRAP -- exception through vector 32 + n, stacking the next PC (PRM 4-188; UM 6.3.5).
    pub(crate) fn op_trap(&mut self, opcode: u16) -> Step {
        let vector = VECTOR_TRAP_BASE + u32::from(opcode & 0xF);
        self.exception(vector, self.pc.wrapping_sub(2), 4, None, true)
    }

    /// TRAPV -- if V, exception through vector 7 (PRM 4-189; UM Table 8-14: 34 clocks).
    pub(crate) fn op_trapv(&mut self, opcode: u16) -> Step {
        if self.sr & V == 0 {
            return self.prefetch();
        }
        // Exception processing starts before the queue refill, which is
        // therefore read in supervisor program space (SST TRAPV).
        let saved = self.enter_supervisor();
        self.prefetch()?;
        // IR takes IRC, but the decoder keeps TRAPV: a fault while the trap
        // is processed stacks TRAPV as its IR (WinUAE's CPU tester, run, T2;
        // MAME 0.285's trpv3 never loads IRD, T3).
        self.opcode = opcode;
        self.exception(VECTOR_TRAPV, self.pc.wrapping_sub(4), 0, Some(saved), true)
    }

    /// DIVU/DIVS by zero: vector 5, stacking the next PC (UM 6.3.5; Table 8-14: 38+).
    ///
    /// The flags are undefined in PRM. The pinned corpus has no case (its
    /// issue #3); WinUAE's 68000 rule (`divbyzero_special`, T2) is used:
    /// DIVS clears N V C and sets Z; DIVU clears V C and sets N and Z from
    /// the high word of the dividend.
    pub(crate) fn divide_by_zero(&mut self, signed: bool, dividend: u32) -> Step {
        let mut ccr = self.sr & (0xFF00 | X);
        if signed {
            ccr |= Z;
        } else if dividend & 0x8000_0000 != 0 {
            ccr |= N;
        } else if dividend & 0xFFFF_0000 == 0 {
            ccr |= Z;
        }
        self.sr = ccr;
        self.exception(VECTOR_ZERO_DIVIDE, self.pc.wrapping_sub(2), 8, None, true)
    }

    /// CHK -- trap through vector 6 if Dn.w < 0 or Dn.w > source (PRM 4-69; UM Table 8-12).
    ///
    /// N, Z, V and C are undefined in PRM; here they are what the microcode
    /// leaves (SST CHK), which is also WinUAE run's rule: Z from Dn, V and C
    /// clear, N set for Dn < 0 and clear for Dn > source. The trap for Dn >
    /// bound takes 38 clocks, the manual's 40 less 2, and the negative-Dn
    /// trap 8 or 10 internal clocks (below): SST CHK.
    pub(crate) fn op_chk(&mut self, opcode: u16) -> Step {
        let bound = sign_extend_16(self.ea_read(ea_kind(opcode), usize::from(opcode & 7), 2)?);
        let value = sign_extend_16(self.r[usize::from((opcode >> 9) & 7)]);
        let mut ccr = self.sr & (0xFF00 | X);
        if value == 0 {
            ccr |= Z;
        }
        if value < 0 {
            ccr |= N;
        }
        self.sr = ccr;
        // The microcode tests the upper bound first. A negative Dn within it
        // costs one more internal step, unless bound - Dn, taken as a 16-bit
        // difference, came out negative (SST CHK: every case agrees).
        if value > bound {
            self.exception(VECTOR_CHK, self.pc.wrapping_sub(2), 8, None, true)
        } else if value < 0 {
            let idle = if (bound - value) & 0x8000 != 0 { 8 } else { 10 };
            self.exception(VECTOR_CHK, self.pc.wrapping_sub(2), idle, None, true)
        } else {
            self.cycles += 6;
            self.prefetch()
        }
    }

    // -- the status register and CCR (PRM 4-20, 4-104, 4-155, 4-123, 6-*) --------

    /// Re-read the queue from the next instruction after SR or CCR is written.
    fn refill_after_status(&mut self) -> Step {
        self.jump(self.pc.wrapping_sub(2))
    }

    fn write_status(&mut self, value: u16, idle: u32) -> Step {
        self.cycles += idle;
        self.set_sr_internal(value);
        self.refill_after_status()
    }

    fn write_ccr(&mut self, value: u16, idle: u32) -> Step {
        self.cycles += idle;
        self.set_ccr(value);
        self.refill_after_status()
    }

    /// ORI to CCR -- CCR <- CCR OR #data (PRM 4-155; UM Table 8-12: 20 clocks).
    pub(crate) fn op_ori_to_ccr(&mut self, _opcode: u16) -> Step {
        let data = self.immediate(1)? as u16;
        self.write_ccr((self.sr | data) & CCR_BITS, 8)
    }

    /// ANDI to CCR -- CCR <- CCR AND #data (PRM 4-20; UM Table 8-12: 20 clocks).
    pub(crate) fn op_andi_to_ccr(&mut self, _opcode: u16) -> Step {
        let data = self.immediate(1)? as u16;
        self.write_ccr(self.sr & data & CCR_BITS, 8)
    }

    /// EORI to CCR -- CCR <- CCR XOR #data (PRM 4-104; UM Table 8-12: 20 clocks).
    pub(crate) fn op_eori_to_ccr(&mut self, _opcode: u16) -> Step {
        let data = self.immediate(1)? as u16;
        self.write_ccr((self.sr ^ data) & CCR_BITS, 8)
    }

    /// ORI to SR -- SR <- SR OR #data, privileged (PRM 6-27; UM Table 8-12: 20 clocks).
    pub(crate) fn op_ori_to_sr(&mut self, _opcode: u16) -> Step {
        if self.sr & S == 0 {
            return self.privilege_violation();
        }
        let data = self.immediate(2)? as u16;
        self.write_status(self.sr | data, 8)
    }

    /// ANDI to SR -- SR <- SR AND #data, privileged (PRM 6-2; UM Table 8-12: 20 clocks).
    pub(crate) fn op_andi_to_sr(&mut self, _opcode: u16) -> Step {
        if self.sr & S == 0 {
            return self.privilege_violation();
        }
        let data = self.immediate(2)? as u16;
        self.write_status(self.sr & data, 8)
    }

    /// EORI to SR -- SR <- SR XOR #data, privileged (PRM 6-6; UM Table 8-12: 20 clocks).
    pub(crate) fn op_eori_to_sr(&mut self, _opcode: u16) -> Step {
        if self.sr & S == 0 {
            return self.privilege_violation();
        }
        let data = self.immediate(2)? as u16;
        self.write_status(self.sr ^ data, 8)
    }

    /// MOVE to CCR -- CCR <- low byte of the source word (PRM 4-123; UM Table 8-12).
    pub(crate) fn op_move_to_ccr(&mut self, opcode: u16) -> Step {
        let value = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), 2)?;
        self.write_ccr(value as u16, 4)
    }

    /// MOVE to SR -- SR <- source word, privileged (PRM 6-19; UM Table 8-12).
    pub(crate) fn op_move_to_sr(&mut self, opcode: u16) -> Step {
        if self.sr & S == 0 {
            return self.privilege_violation();
        }
        let value = self.ea_read(ea_kind(opcode), usize::from(opcode & 7), 2)?;
        self.write_status(value as u16, 4)
    }

    /// MOVE from SR -- destination <- SR; not privileged on the 68000 (PRM 6-17; UM 6.3.7).
    ///
    /// The memory operand is read before it is written, as the chip does;
    /// the 2 idle clocks of the register form and the PC an address error
    /// stacks: SST MOVEfromSR.
    pub(crate) fn op_move_from_sr(&mut self, opcode: u16) -> Step {
        let kind = ea_kind(opcode);
        let register = usize::from(opcode & 7);
        if kind == DN {
            self.prefetch()?;
            self.cycles += 2;
            self.write_register(register, 2, u32::from(self.sr));
            return Ok(());
        }
        let (address, _) = self.ea_fetch(kind, register, 2)?; // read first, as the chip does
        self.prefetch()?;
        self.write_word(address, u32::from(self.sr))
    }

    /// MOVE USP -- An <-> USP, privileged (PRM 6-21; UM Table 8-12: 4 clocks).
    pub(crate) fn op_move_usp(&mut self, opcode: u16) -> Step {
        if self.sr & S == 0 {
            return self.privilege_violation();
        }
        let an = 8 + usize::from(opcode & 7);
        if opcode & 8 != 0 {
            self.r[an] = self.other_sp;
        } else {
            self.other_sp = self.r[an];
        }
        self.prefetch()
    }

    // -- returns that restore state --------------------------------------------

    /// RTE -- SR <- (SP)+, PC <- (SP)+, privileged (PRM 6-84; UM Table 8-12: 20 clocks).
    pub(crate) fn op_rte(&mut self, _opcode: u16) -> Step {
        if self.sr & S == 0 {
            return self.privilege_violation();
        }
        let sp = self.r[15];
        let status = self.read_word(sp)?;
        let high = self.read_word(sp.wrapping_add(2))?;
        self.r[15] = sp.wrapping_add(6);
        let pc = (high << 16) | self.read_word(sp.wrapping_add(4))?;
        self.set_sr_internal(status as u16);
        self.jump(pc)
    }

    // -- STOP and RESET ------------------------------------------------------------

    /// STOP -- SR <- #data, then stop until an interrupt or reset (PRM 6-85; UM Table 8-12).
    ///
    /// The immediate is taken from IRC without a refill and the queue is
    /// left as it was, so a stopped CPU resumes at the instruction after
    /// STOP: SST STOP.
    pub(crate) fn op_stop(&mut self, _opcode: u16) -> Step {
        if self.sr & S == 0 {
            return self.privilege_violation();
        }
        let data = self.irc;
        self.cycles += 4;
        self.set_sr_internal(data);
        self.stopped = true;
        Ok(())
    }

    /// RESET -- pulse the RESET line, privileged (PRM 6-83; UM Table 8-12: 132 clocks).
    ///
    /// The pulse is 124 clocks; the instruction takes 132 in all (UM Table
    /// 8-12): 128 before the closing prefetch, as SST RESET records. The
    /// processor itself is not reset. A host that wants to see the pulse
    /// implements `Bus::reset_devices`.
    pub(crate) fn op_reset(&mut self, _opcode: u16) -> Step {
        if self.sr & S == 0 {
            return self.privilege_violation();
        }
        self.cycles += 128;
        self.bus.reset_devices();
        self.prefetch()
    }
}
