use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::TransformOldReadScope;
use crate::MarrowType;
use marrow_schema::ReturnPresence;

#[derive(Clone, Debug)]
pub(super) struct NameScope {
    frames: Vec<HashMap<String, ScopedBinding>>,
    next_binding: u32,
    return_presence: ReturnPresence,
    transform_old: Option<TransformOldBinding>,
    source_file: Option<PathBuf>,
}

impl Default for NameScope {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            next_binding: 0,
            return_presence: ReturnPresence::Always,
            transform_old: None,
            source_file: None,
        }
    }
}

#[derive(Clone, Debug)]
struct ScopedBinding {
    id: u32,
    ty: Option<MarrowType>,
}

#[derive(Clone, Debug)]
struct TransformOldBinding {
    binding: u32,
    resource: String,
}

impl NameScope {
    pub(super) fn for_function(function: &crate::CheckedFunction, source_file: &Path) -> Self {
        let mut scope = Self {
            return_presence: function.return_presence,
            source_file: Some(source_file.to_path_buf()),
            ..Self::default()
        };
        scope.push_frame();
        for param in &function.params {
            scope.bind_with_type(&param.name, Some(param.ty.clone()));
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
                let binding = scope.bind_with_type(name, frame.get(name).cloned());
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
        self.bind_with_type(name, None)
    }

    pub(super) fn bind_with_type(&mut self, name: &str, ty: Option<MarrowType>) -> u32 {
        let id = self.next_binding;
        self.next_binding += 1;
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name.to_string(), ScopedBinding { id, ty });
        }
        id
    }

    pub(super) fn lookup(&self, name: &str) -> Option<u32> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).map(|binding| binding.id))
    }

    pub(super) fn lookup_type(&self, name: &str) -> Option<&MarrowType> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).and_then(|binding| binding.ty.as_ref()))
    }

    pub(super) fn transform_old_resource(&self) -> Option<&str> {
        let old = self.transform_old.as_ref()?;
        (self.lookup("old")? == old.binding).then_some(old.resource.as_str())
    }

    pub(super) fn source_file(&self) -> &Path {
        self.source_file.as_deref().unwrap_or_else(|| Path::new(""))
    }

    pub(super) fn return_presence(&self) -> ReturnPresence {
        self.return_presence
    }
}
