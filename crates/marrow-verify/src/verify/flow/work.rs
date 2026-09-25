//! The flow-verification work tally the budget tests read.
//!
//! `check_flow` and the operand stack call these hooks at their charge sites. Test
//! builds count into a thread-local `FlowWork`; every other build compiles them to
//! nothing. Verification runs on the calling thread, so a test that clears the tally
//! before `crate::verify` and takes it afterwards reads exactly that call's work.

#[cfg(test)]
pub(super) use counted::{FlowWork, cells, minted, queued, run, take_flow_work};
#[cfg(not(test))]
pub(super) use uncounted::{cells, minted, queued, run};

#[cfg(test)]
mod counted {
    use std::cell::Cell;

    /// Work one or more flow checks performed on this thread since the last take.
    #[derive(Clone, Copy, Debug, Default)]
    pub(in super::super) struct FlowWork {
        /// Operand-stack cells the checker handled on its charged path.
        pub(in super::super) stack_cells: usize,
        /// Distinct operand-stack prefixes minted.
        pub(in super::super) nodes_minted: usize,
        /// Region runs: one per worklist pop.
        pub(in super::super) runs: usize,
        /// Boundaries queued while already waiting in the worklist.
        pub(in super::super) queued_while_pending: usize,
    }

    thread_local! {
        static WORK: Cell<FlowWork> = Cell::new(FlowWork::default());
    }

    fn charge(update: impl FnOnce(&mut FlowWork)) {
        WORK.with(|work| {
            let mut tally = work.get();
            update(&mut tally);
            work.set(tally);
        });
    }

    /// Return this thread's tally and reset it.
    pub(in super::super) fn take_flow_work() -> FlowWork {
        WORK.with(Cell::take)
    }

    pub(in super::super) fn run() {
        charge(|work| work.runs += 1);
    }

    pub(in super::super) fn queued(worklist: &[usize], successor: usize) {
        if worklist.contains(&successor) {
            charge(|work| work.queued_while_pending += 1);
        }
    }

    pub(in super::super) fn cells(count: usize) {
        charge(|work| work.stack_cells += count);
    }

    pub(in super::super) fn minted() {
        charge(|work| work.nodes_minted += 1);
    }
}

#[cfg(not(test))]
mod uncounted {
    #[inline]
    pub(in super::super) fn run() {}

    #[inline]
    pub(in super::super) fn queued(_worklist: &[usize], _successor: usize) {}

    #[inline]
    pub(in super::super) fn cells(_count: usize) {}

    #[inline]
    pub(in super::super) fn minted() {}
}
