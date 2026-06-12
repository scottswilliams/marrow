use std::collections::HashMap;

use super::TransformOldReadScope;

#[derive(Debug, Default)]
pub(super) struct NameScope {
    frames: Vec<HashMap<String, u32>>,
    next_binding: u32,
    transform_old: Option<TransformOldBinding>,
}

#[derive(Debug)]
struct TransformOldBinding {
    binding: u32,
    resource: String,
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

    pub(super) fn for_transform(resource: &str) -> Self {
        let mut scope = Self::default();
        scope.push_frame();
        let binding = scope.bind("old");
        scope.transform_old = Some(TransformOldBinding {
            binding,
            resource: resource.to_string(),
        });
        scope
    }

    pub(super) fn from_type_scope(
        type_scope: &[HashMap<String, crate::MarrowType>],
        transform_old: Option<TransformOldReadScope<'_>>,
    ) -> Self {
        let mut scope = Self::default();
        for (frame_index, frame) in type_scope.iter().enumerate() {
            scope.push_frame();
            let mut names: Vec<&str> = frame.keys().map(String::as_str).collect();
            names.sort_unstable();
            for name in names {
                let binding = scope.bind(name);
                if let Some(old) = transform_old
                    && name == "old"
                    && old.frame == frame_index
                {
                    scope.transform_old = Some(TransformOldBinding {
                        binding,
                        resource: old.resource.to_string(),
                    });
                }
            }
        }
        scope
    }

    pub(super) fn push_frame(&mut self) {
        self.frames.push(HashMap::new());
    }

    pub(super) fn pop_frame(&mut self) {
        self.frames.pop();
    }

    pub(super) fn bind(&mut self, name: &str) -> u32 {
        let id = self.next_binding;
        self.next_binding += 1;
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name.to_string(), id);
        }
        id
    }

    pub(super) fn lookup(&self, name: &str) -> Option<u32> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).copied())
    }

    pub(super) fn transform_old_resource(&self) -> Option<&str> {
        let old = self.transform_old.as_ref()?;
        (self.lookup("old")? == old.binding).then_some(old.resource.as_str())
    }
}
