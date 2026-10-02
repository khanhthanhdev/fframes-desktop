/// Counts acknowledged painted frames, rather than conversions or render calls.
#[derive(Default)]
pub struct PresentationStress {
    pub run: u64,
    pub confirmed: usize,
    pub active: bool,
    waiting: Option<usize>,
}

impl PresentationStress {
    pub const TARGET: usize = 1_000;

    pub fn start(&mut self) {
        self.run += 1;
        self.confirmed = 0;
        self.active = true;
        self.waiting = None;
    }

    pub fn admit(&mut self) -> Option<(u64, usize)> {
        if !self.active || self.waiting.is_some() {
            return None;
        }
        self.waiting = Some(self.confirmed);
        Some((self.run, self.confirmed))
    }

    pub fn acknowledge(&mut self, run: u64, sequence: usize) -> bool {
        if !self.active || self.run != run || self.waiting != Some(sequence) {
            return false;
        }
        self.waiting = None;
        self.confirmed += 1;
        self.active = self.confirmed < Self::TARGET;
        true
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.waiting = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_only_matching_completions_and_finishes_at_one_thousand() {
        let mut stress = PresentationStress::default();
        stress.start();
        for sequence in 0..PresentationStress::TARGET {
            let (run, admitted) = stress.admit().unwrap();
            assert_eq!(admitted, sequence);
            assert!(stress.admit().is_none());
            assert!(!stress.acknowledge(run + 1, sequence));
            assert_eq!(stress.confirmed, sequence);
            assert!(stress.acknowledge(run, sequence));
            assert!(!stress.acknowledge(run, sequence));
        }
        assert!(!stress.active);
        assert!(stress.admit().is_none());
    }
    #[test]
    fn stopped_and_previous_runs_cannot_complete_new_run() {
        let mut stress = PresentationStress::default();
        stress.start();
        let old = stress.admit().unwrap();
        stress.stop();
        assert!(!stress.acknowledge(old.0, old.1));
        stress.start();
        stress.admit();
        assert!(!stress.acknowledge(old.0, old.1));
        assert_eq!(stress.confirmed, 0);
    }
}
