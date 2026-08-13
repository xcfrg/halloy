use std::collections::{HashMap, hash_map};
use std::hash::Hash;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::WatchStream;

use super::metadata::ReadMarker;
use super::{Kind, Model, insert_message, model};
use crate::capabilities::LabeledResponseContext;
use crate::{Config, Notification, Server, client, message};

pub enum Message {
    AddMessages(Kind, Vec<message::MessageWithContext>),
    LoadMessages(Kind, message::Limit),
    UpdateReadMarker(Kind, ReadMarker),
}

pub enum Event {
    Model(model::Message),
    Notification(Notification),
}

#[derive(Debug, Default)]
pub struct Manager {
    storage: HashMap<Kind, Storage>,
    filters: Vec<Filter>,
    reroute_rules: RerouteRules,
}

impl Manager {
    async fn get(&mut self, kind: Kind) -> (Storage, Vec<Event>) {
        let mut events = Vec::new();

        let kind_storage = match self.storage.entry(kind) {
            hash_map::Entry::Occupied(mut entry) => entry.get_mut(),
            hash_map::Entry::Vacant(entry) => {
                let (storage, model) =
                    Storage::load_or_default(entry.key().clone()).await;

                events.push(Event::Model(model::Message::Open(
                    entry.key().clone(),
                    model,
                )));

                entry.insert(storage)
            }
        };

        (kind_storage, events)
    }

    pub async fn update(
        &mut self,
        message: Message,
        config: &Config,
    ) -> Vec<Event> {
        match message {
            Message::AddMessages(kind, messages) => {
                let (storage, mut events) = self.get(kind).await;

                events.extend(storage.add_messages(messages, config).await);

                events
            }
            Message::LoadMessages(kind, limit) => {
                let (storage, mut events) = self.get(kind).await;

                events.push(storage.get_messages(limit).await);

                events
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Storage {
    kind: Kind,
    show_in_sidebar: bool,
    read_marker: Option<ReadMarker>,
    display_read_marker: Option<ReadMarker>,
    max_triggers_unread: Option<DateTime<Utc>>,
    max_triggers_highlight: Option<DateTime<Utc>>,
    last_seen: HashMap<Nick, DateTime<Utc>>,
    read_cache: ReadCache,
    write_buffer: WriteBuffer,
}

pub impl Storage {
    pub async fn load_or_default(
        kind: Kind,
        model_limit: Option<message::Limit>,
    ) -> (Self, Model) {
        if let Some((storage, model_update)) =
            Storage::load(kind, model_limit).await
        {
            let mut model = Model::from(&storage);

            if let Some(model_update) = model_update {
                model.update(model_update);
            }

            (storage, model)
        } else {
            let storage = Self::from(kind);
            let model = Model::from(&storage);

            (storage, model)
        }
    }

    pub async fn load(
        kind: Kind,
        model_limit: Option<message::Limit>,
    ) -> (Self, model::Update) {
    }

    pub async fn add_messages(
        &mut self,
        messages: Vec<message::MessageWithContext>,
        config: &Config,
    ) -> Vec<Event> {
    }

    pub async fn get_messages(&mut self) -> Vec<Event> {}
}
