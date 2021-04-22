use rustc_index::bit_set::BitSet;
use rustc_middle::mir::{
    visit::Visitor, BasicBlock, Body, HasLocalDecls, Local, Location, Operand, Place, Rvalue,
    Statement, StatementKind, Terminator,
};
use rustc_mir::dataflow::{AnalysisDomain, Forward, GenKill, GenKillAnalysis};

/// A dataflow analysis that tracks whether a value may carry a taint.
///
/// Taints are introduced through sources, and consumed by sinks.
/// Ideally, a sink never consumes a tainted value - this should result in an error.
pub struct MaybeTaintedLocals;

impl<'tcx> AnalysisDomain<'tcx> for MaybeTaintedLocals {
    type Domain = BitSet<Local>;
    const NAME: &'static str = "MaybeTaintedLocals";

    type Direction = Forward;

    fn bottom_value(&self, body: &Body<'tcx>) -> Self::Domain {
        // bottom = untainted
        BitSet::new_empty(body.local_decls().len())
    }

    fn initialize_start_block(&self, _body: &Body<'tcx>, _state: &mut Self::Domain) {
        // Locals start out being untainted
    }
}

impl<'tcx> GenKillAnalysis<'tcx> for MaybeTaintedLocals {
    type Idx = Local;

    fn statement_effect(
        &self,
        trans: &mut impl GenKill<Self::Idx>,
        statement: &Statement<'tcx>,
        location: Location,
    ) {
        self.transfer_function(trans)
            .visit_statement(statement, location);
    }

    fn terminator_effect(
        &self,
        trans: &mut impl GenKill<Self::Idx>,
        terminator: &Terminator<'tcx>,
        location: Location,
    ) {
        self.transfer_function(trans)
            .visit_terminator(terminator, location);
    }

    fn call_return_effect(
        &self,
        _trans: &mut impl GenKill<Self::Idx>,
        _block: BasicBlock,
        _func: &Operand<'tcx>,
        _args: &[Operand<'tcx>],
        _return_place: Place<'tcx>,
    ) {
        todo!()
    }
}

impl<'a> MaybeTaintedLocals {
    fn transfer_function<T>(&self, trans: &'a mut T) -> TransferFunction<'a, T> {
        TransferFunction { trans }
    }
}

struct TransferFunction<'a, T> {
    trans: &'a mut T,
}

impl<'a, T> TransferFunction<'a, T>
where
    T: GenKill<Local>,
{
    fn propagate(&mut self, old: Local, new: Local) {
        if self.is_tainted(old) {
            self.trans.gen(new);
        } else {
            self.trans.kill(new);
        }
    }

    fn is_tainted(&mut self, elem: Local) -> bool {
        let set = self.get_set();
        set.contains(elem)
    }

    /// Forget you ever saw this
    fn get_set(&mut self) -> &BitSet<Local> {
        unsafe { &*(self.trans as *mut T as *const BitSet<Local>) }
    }

    fn handle_assignment(&mut self, assignment: &(Place, Rvalue)) {
        let (target, ref rval) = *assignment;
        match rval {
            // If we assign a constant to a place, the place is clean.
            Rvalue::Use(Operand::Constant(_)) => self.trans.kill(target.local),

            // Otherwise we propagate the taint
            Rvalue::Use(Operand::Copy(f) | Operand::Move(f)) => {
                self.propagate(f.local, target.local);
            }

            Rvalue::BinaryOp(_, ref b) => {
                let (ref o1, ref o2) = **b;
                match (o1, o2) {
                    (Operand::Constant(_), Operand::Constant(_)) => self.trans.kill(target.local),
                    (Operand::Copy(a) | Operand::Move(a), Operand::Copy(b) | Operand::Move(b)) => {
                        if self.is_tainted(a.local) || self.is_tainted(b.local) {
                            self.trans.gen(target.local);
                        } else {
                            self.trans.kill(target.local);
                        }
                    }
                    (Operand::Copy(p) | Operand::Move(p), Operand::Constant(_))
                    | (Operand::Constant(_), Operand::Copy(p) | Operand::Move(p)) => {
                        if self.is_tainted(p.local) {
                            self.trans.gen(target.local);
                        } else {
                            self.trans.kill(target.local);
                        }
                    }
                }
            }
            Rvalue::UnaryOp(_, Operand::Move(p) | Operand::Copy(p)) => {
                self.propagate(p.local, target.local);
            }
            _ => {}
        }
    }
}

impl<'tcx, T> Visitor<'tcx> for TransferFunction<'_, T>
where
    T: GenKill<Local>,
{
    fn visit_statement(&mut self, statement: &Statement<'tcx>, _location: Location) {
        if let StatementKind::Assign(ref assignment) = statement.kind {
            self.handle_assignment(assignment);
        }
    }

    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, _location: Location) {
        match &terminator.kind {
            rustc_middle::mir::TerminatorKind::Goto { target: _ } => {}
            rustc_middle::mir::TerminatorKind::SwitchInt {
                discr: _discr,
                switch_ty: _switch_ty,
                targets: _targets,
            } => {}
            rustc_middle::mir::TerminatorKind::Return => {}
            rustc_middle::mir::TerminatorKind::Call {
                func: _func,
                args: _args,
                destination: _destination,
                cleanup: _cleanup,
                from_hir_call: _from_hir_call,
                fn_span: _fn_span,
            } => {}
            rustc_middle::mir::TerminatorKind::Assert {
                cond: _cond,
                expected: _expected,
                msg: _msg,
                target: _target,
                cleanup: _cleanup,
            } => {}
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propagate() {
        let one = Local::from_u32(1);
        let two = Local::from_u32(2);
        let three = Local::from_u32(3);
        let mut set: BitSet<Local> = BitSet::new_empty(4);
        set.insert(one);

        let mut trans = TransferFunction { trans: &mut set };

        trans.propagate(one, two);
        trans.propagate(three, one);

        assert!(set.contains(two));
        assert!(!set.contains(one));
    }
}
