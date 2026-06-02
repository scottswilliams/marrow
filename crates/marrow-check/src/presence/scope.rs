use std::collections::HashMap;

#[derive(Debug, Default)]
pub(super) struct NameScope {
    frames: Vec<HashMap<String, u32>>,
    next_binding: u32,
}

impl NameScope {
    pub(super) fn for_function(function: &crate::CheckedFunction) -> Self {
        let mut scope = Self::default();
        scope.push_frame();
        for param in &function.params {
            scope.bind(&param.name);
        }
        scope
    }

    pub(super) fn push_frame(&mut self) {
        self.frames.push(HashMap::new());
    }

    pub(super) fn pop_frame(&mut self) {
        self.frames.pop();
    }

    pub(super) fn bind(&mut self, name: &str) {
        let id = self.next_binding;
        self.next_binding += 1;
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name.to_string(), id);
        }
    }

    pub(super) fn lookup(&self, name: &str) -> Option<u32> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).copied())
    }
}
