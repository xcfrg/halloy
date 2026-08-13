use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, watch};

use super::Kind;
use super::metadata::ReadMarker;
use crate::{Config, message};

#[derive(Debug, Clone)]
pub enum Message {
    Insert(Kind, Model),
    Update(Kind, Update),
    Remove(Kind),
}

#[derive(Debug)]
pub struct Manager {
    models: HashMap<Kind, Model>,
}

impl Manager {
    pub fn view(&self, kind: &Kind, config: &Config) -> Option<View<'_>> {
        self.models.get(kind).map(|model| model.view(config))
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Insert(kind, model) => {
                self.models.insert(kind, model);
            }
            Message::Update(kind, update) => {
                if let Some(model) = self.models.get_mut(&kind) {
                    model.update(update);
                }
            }
            Message::Remove(kind) => self.models.remove(&kind),
        }
    }
}

pub enum Update {
    Base {
        show_in_sidebar: bool,
        read_marker: Option<ReadMarker>,
        display_read_marker: Option<ReadMarker>,
        max_triggers_unread: Option<DateTime<Utc>>,
        max_triggers_highlight: Option<DateTime<Utc>>,
    },
    Pane(pane),
}

#[derive(Debug)]
pub struct Model {
    show_in_sidebar: bool,
    read_marker: Option<ReadMarker>,
    display_read_marker: Option<ReadMarker>,
    max_triggers_unread: Option<DateTime<Utc>>,
    max_triggers_highlight: Option<DateTime<Utc>>,
    pane: Pane,
}

#[derive(Debug)]
pub enum Pane {
    Open {
        has_more_older_messages: bool,
        has_more_newer_messages: bool,
        messages: Vec<message::MessageDisplay>, // sorted by MessageDisplay.message.server_time
        limit: message::Limit,
    },
    Closed,
}

impl From<&Storage> for Model {}

pub impl Model {
    pub fn view(
        &self,
        request_limit: message::Limit,
        config: &Config,
    ) -> View<'_> {
        match &self.pane {
            Open {
                has_more_older_messages,
                has_more_newer_messages,
                messages,
                limit,
            } => {
                let processed = process_messages(messages, config);

                let split_at =
                    self.display_read_marker.map_or(0, |display_read_marker| {
                        processed
                            .iter()
                            .rev()
                            .position(|message| {
                                message.server_time()
                                    <= display_read_marker.date_time()
                            })
                            .map_or_else(
                                || 0, // Backlog is before current, limited view of messages
                                |position| processed.len() - position,
                            )
                    });

                let (old, new) = processed.split_at(split_at);

                View {
                    old_messages: old.to_vec(),
                    new_messages: new.to_vec(),
                    has_more_older_messages: *has_more_older_messages,
                    has_more_newer_messages: *has_more_newer_messages,
                    loading: request_limit != limit,
                }
            }
            Closed => View::default(),
        }
    }

    pub fn update(&mut self, update: Update) -> Event {
        match update {
            Update::Base {
                show_in_sidebar,
                read_marker,
                display_read_marker,
                max_triggers_unread,
                max_triggers_highlight,
            } => {
                self.show_in_sidebar = show_in_sidebar;
                self.read_marker = read_marker;
                self.display_read_marker = display_read_marker;
                self.max_triggers_unread = max_triggers_unread;
                self.max_triggers_highlight = max_triggers_highlight;
            }
            Update::Pane(pane) => {
                self.pane = pane;
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct View<'a> {
    pub old_messages: Vec<&'a message::MessageDisplay>,
    pub new_messages: Vec<&'a message::MessageDisplay>,
    pub has_more_older_messages: bool,
    pub has_more_newer_messages: bool,
    pub loading: bool,
}

/// MessageDisplay processing that must happen at view-time (e.g. filtering
/// messages based on how long ago they were received) is done here.  All
/// other processing is done by Storage.
fn process_messages(
    &messages: Vec<message::MessageDisplay>,
    config: &Config,
) -> Vec<&message::MessageDisplay> {
    let current_time = Utc::now();

    messages
        .iter()
        .flat_map(|message| {
            if message.blocked {
                None
            } else if message
                .can_condense(&config.buffer.server_messages.condense)
            {
                if message.expanded {
                    Some(message)
                } else {
                    message.condensed.as_ref().and_then(|condensed_message| {
                        (!condensed_message.text().is_empty())
                            .then_some(condensed_message.as_ref())
                    })
                }
            } else {
                match message.source() {
                    message::Source::Internal(
                        message::source::Internal::Status(status),
                    ) => {
                        if !config.buffer.internal_messages.enabled(status) {
                            return None;
                        } else if let Some(seconds) =
                            config.buffer.internal_messages.smart(status)
                        {
                            return (!smart_filter_internal_message(
                                message,
                                &seconds,
                                &current_time,
                            ))
                            .then_some(message);
                        }

                        Some(message)
                    }
                    _ => Some(message),
                }
            }
        })
        .collect::<Vec<_>>()
}

fn smart_filter_internal_message(
    message: &message::MessageDisplay,
    seconds: &i64,
    current_time: &DateTime<Utc>,
) -> bool {
    let duration_seconds = current_time
        .signed_duration_since(message.server_time())
        .num_seconds();

    duration_seconds > *seconds
}
