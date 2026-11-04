// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `FluentProvider.cs` and `FluentBundle.cs`.

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use rustora_fs::ReadFs;

/// The messages of a mod.
pub struct Messages {
    bundle: FluentBundle<FluentResource>,
}

/// One argument value of a message.
pub enum MessageArg<'a> {
    Text(&'a str),
    Number(i64),
}

impl Messages {
    pub fn new(files: &[String], fs: &dyn ReadFs) -> Messages {
        let culture = "en".parse().expect("language identifier");
        let mut bundle = FluentBundle::new(vec![culture]);
        bundle.set_use_isolating(false);
        for file in files {
            let Some(data) = fs.try_open(file) else {
                continue;
            };
            let text = String::from_utf8_lossy(&data).into_owned();
            let resource = FluentResource::try_new(text).unwrap_or_else(|(resource, _)| resource);
            bundle.add_resource_overriding(resource);
        }
        Messages { bundle }
    }

    /// Returns the message, or the key itself if it does not exist, like `FluentBundle.GetMessage`.
    pub fn get(&self, key: &str) -> String {
        self.get_with(key, &[])
    }

    pub fn get_with(&self, key: &str, args: &[(&str, MessageArg)]) -> String {
        self.try_get(key, args).unwrap_or_else(|| key.to_string())
    }

    pub fn try_get(&self, key: &str, args: &[(&str, MessageArg)]) -> Option<String> {
        let (name, attribute) = match key.split_once('.') {
            Some((name, attribute)) => (name, Some(attribute)),
            None => (key, None),
        };
        let message = self.bundle.get_message(name)?;
        let pattern = match attribute {
            Some(attribute) => message.get_attribute(attribute)?.value(),
            None => message.value()?,
        };
        let mut fluent_args = FluentArgs::new();
        for (name, value) in args {
            let value: FluentValue = match value {
                MessageArg::Text(t) => FluentValue::from(t.to_string()),
                MessageArg::Number(n) => FluentValue::from(*n),
            };
            fluent_args.set(name.to_string(), value);
        }
        let mut errors = Vec::new();
        Some(
            self.bundle
                .format_pattern(pattern, (!args.is_empty()).then_some(&fluent_args), &mut errors)
                .into_owned(),
        )
    }
}
