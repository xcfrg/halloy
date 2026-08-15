use std::collections::{HashMap, hash_map};

use chrono::{DateTime, Local, NaiveDate, Utc};
use itertools::Itertools;

use super::filter::{Filter, FilterChain};
use super::reroute::RerouteRules;
use super::{
    Kind, Model, ReadMarker, model, smart_filter_internal_message,
    smart_filter_message, smart_filter_repeat,
};
use crate::message::MessageReferences;
use crate::user::Nick;
use crate::{Config, Notification, client, config, message};

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
    async fn get(&mut self, kind: Kind) -> (&mut Storage, Vec<Event>) {
        let mut events = Vec::new();

        let kind_storage = match self.storage.entry(kind) {
            hash_map::Entry::Occupied(entry) => entry.into_mut(),
            hash_map::Entry::Vacant(entry) => {
                let (storage, model) =
                    Storage::load_or_default(entry.key().clone(), None).await;

                events.push(Event::Model(model::Message::Insert(
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
            Message::UpdateReadMarker(kind, read_marker) => {
                let (storage, mut events) = self.get(kind).await;

                events.push(storage.update_read_marker(read_marker));

                events
            }
        }
    }

    // Block, condense, and populate reply-previews for history's messages
    pub fn process_history(
        &mut self,
        kind: &Kind,
        clients: &client::Map,
        buffer_config: &config::Buffer,
    ) -> Option<Event> {
        self.storage.get_mut(&kind).and_then(|kind_storage| {
            let event = process_messages(
                &kind,
                self.read_cache.messages,
                FilterChain::borrow(&self.filters),
                clients,
                buffer_config,
            );

            log::debug!("processed messages in {kind}");

            event
        })
    }
}

#[derive(Debug)]
pub struct Storage {
    kind: Kind,
    show_in_sidebar: bool,
    latest_triggers_unread: Option<DateTime<Utc>>,
    latest_triggers_highlight: Option<DateTime<Utc>>,
    display_read_marker: Option<ReadMarker>,
    read_marker: Option<ReadMarker>,
    chathistory_references: Option<MessageReferences>,
    read_cache: ReadCache,
    write_buffer: WriteBuffer,
    last_seen: HashMap<Nick, DateTime<Utc>>,
}

impl From<Kind> for Storage {
    fn from(kind: Kind) -> Self {
        Self {
            kind,
            show_in_sidebar: false,
            latest_triggers_unread: None,
            latest_triggers_highlight: None,
            display_read_marker: None,
            read_marker: None,
            chathistory_references: None,
            read_cache: ReadCache::default(),
            write_buffer: WriteBuffer::default(),
            last_seen: HashMap::new(),
        }
    }
}

impl Storage {
    pub async fn load_or_default(
        kind: Kind,
        model_limit: Option<message::Limit>,
    ) -> (Self, Model) {
        let mut storage = Self::from(kind);

        let mut model = Model::from(&storage);

        if let Some(model_update) = storage.load(model_limit).await {
            model.update(model_update);
        }

        (storage, model)
    }

    pub async fn load(
        &mut self,
        model_limit: Option<message::Limit>,
    ) -> Option<model::Update> {
        // Need help: we need to load values for read_marker and
        // chathistory_references (display_read_marker should be set equal to
        // read_marker)
        self.read_cache.load(model_limit)
    }

    pub async fn add_messages(
        &mut self,
        messages: Vec<message::MessageWithContext>,
        config: &Config,
    ) -> Vec<Event> {
        self.write_buffer.add_messages(messages, config)
    }

    pub async fn get_messages(&mut self, model_limit: message::Limit) -> Event {
    }

    pub fn update_read_marker(&mut self, read_marker: ReadMarker) -> Event {
        self.display_read_marker =
            (self.display_read_marker).max(Some(read_marker));
        self.read_marker = Some(read_marker);

        Event::Model(model::Message::Update(
            self.kind.clone(),
            vec![self.model_base_update()],
        ))
    }

    pub fn model_base_update(&self) -> model::Update {
        model::Update::Base {
            show_in_sidebar: self.show_in_sidebar,
            read_marker: self.read_marker,
            display_read_marker: self.display_read_marker,
            latest_triggers_unread: self.latest_triggers_unread,
            latest_triggers_highlight: self.latest_triggers_highlight,
        }
    }
}

#[derive(Debug, Default)]
pub struct ReadCache {
    messages: Vec<message::MessageDisplay>,
    limit: Option<message::Limit>,
}

impl ReadCache {
    pub fn load(
        &mut self,
        model_limit: Option<message::Limit>,
    ) -> Vec<model::Update> {
        // Need help, wee need to load messages based on model limit (or lack
        // thereof), then produce a model::Update based on the loaded messages
    }
}

#[derive(Debug, Default)]
pub struct WriteBuffer {
    messages: Vec<message::Message>,
}

impl WriteBuffer {
    pub fn add_messages(
        &mut self,
        messages: Vec<message::MessageWithContext>,
        config: &Config,
    ) -> Vec<Event> {
        // Need help, I think we want to essentially want to perform something
        // like history::append here, but maybe buffer the writes based on the
        // last requested message::Limit?
    }
}

/// Process `MessageDisplay`s, determining their block, condense, and
/// reply-previews.
fn process_messages(
    kind: &Kind,
    messages: &mut [message::MessageDisplay],
    filter_chain: FilterChain,
    clients: &client::Map,
    buffer_config: &config::Buffer,
) {
    block_messages(kind, messages, filter_chain, clients, buffer_config);

    condense_messages(messages, buffer_config);

    populate_messages_reply_previews(messages);
}

/// Determine which messages should be blocked (hidden).
fn block_messages(
    kind: &Kind,
    messages: &mut [message::MessageDisplay],
    filter_chain: FilterChain,
    clients: &client::Map,
    buffer_config: &config::Buffer,
) {
    let current_time = Utc::now();

    let mut last_seen = HashMap::<Nick, DateTime<Utc>>::new();
    let mut last_away = HashMap::<Nick, DateTime<Utc>>::new();

    messages.iter_mut().for_each(|message| {
        message.blocked = false;

        if message.has_redaction()
            && !buffer_config.redaction.display.is_visible()
        {
            message.blocked = true;
        } else {
            match message.source() {
                message::Source::Server(source) => {
                    let server = if let Some(server) = kind.server() {
                        Some(server)
                    } else if let message::Target::Highlights {
                        server, ..
                    }
                    | message::Target::ChannelMonitor {
                        server,
                        ..
                    } = message.target()
                    {
                        Some(server)
                    } else {
                        None
                    };

                    let casemapping =
                        clients.get_maybe_server_casemapping_or_default(server);

                    // Check if target is included/excluded.
                    let target_ref = match message.target() {
                        message::Target::Channel { channel }
                        | message::Target::ChannelMonitor { channel, .. }
                        | message::Target::Highlights { channel, .. } => {
                            Some(channel.as_target_ref())
                        }

                        message::Target::Query { query } => {
                            Some(query.as_target_ref())
                        }
                        message::Target::Server | message::Target::Logs => None,
                    };

                    let source_kind = source
                        .as_ref()
                        .map(message::source::server::Server::kind);

                    if let Some(target_ref) = target_ref
                        && let Some(server) = server
                        && !buffer_config.server_messages.should_send_message(
                            source.as_ref(),
                            target_ref,
                            server,
                            casemapping,
                        )
                    {
                        message.blocked = true;
                    } else if let Some(seconds) =
                        buffer_config.server_messages.smart(source_kind)
                    {
                        let nick = match source
                            .as_ref()
                            .and_then(|source| source.nick())
                        {
                            Some(nick) => Some(nick.clone()),
                            None => message.plain().and_then(|s| {
                                s.split(' ').nth(1).map(|nick| {
                                    Nick::from_str(nick, casemapping)
                                })
                            }),
                        };

                        if let Some(nick) = nick {
                            match source_kind {
                                Some(message::Kind::Away) => {
                                    message.blocked = smart_filter_repeat(
                                        &message.inner,
                                        &seconds,
                                        last_away.get(&nick),
                                    );

                                    if !message.blocked {
                                        last_away.insert(
                                            nick.clone(),
                                            *message.server_time(),
                                        );
                                    }
                                }
                                _ => {
                                    message.blocked = smart_filter_message(
                                        &message.inner,
                                        &seconds,
                                        last_seen.get(&nick),
                                    );
                                }
                            }
                        }
                    }
                }
                message::Source::User(message_user) => {
                    last_seen.insert(
                        message_user.nickname().to_owned(),
                        *message.server_time(),
                    );
                }
                message::Source::Internal(
                    message::source::Internal::Status(status),
                ) => {
                    if !buffer_config.internal_messages.enabled(status) {
                        message.blocked = true;
                    } else if let Some(seconds) =
                        buffer_config.internal_messages.smart(status)
                    {
                        message.blocked = smart_filter_internal_message(
                            &message.inner,
                            &seconds,
                            &current_time,
                        );
                    }
                }
                _ => (),
            }
        }

        if !message.blocked {
            message.blocked =
                filter_chain.filter_message_of_kind(&message.inner, kind);
        }
    });
}

fn condense_messages(
    messages: &mut [message::MessageDisplay],
    buffer_config: &config::Buffer,
) {
    #[derive(PartialEq)]
    enum CondensationKey {
        Condensable(NaiveDate),
        Singular,
    }

    messages
        .iter_mut()
        .filter(|message| !message.blocked)
        .chunk_by(|message| {
            if message.can_condense(&buffer_config.server_messages.condense) {
                CondensationKey::Condensable(
                    message.server_time().with_timezone(&Local).date_naive(),
                )
            } else {
                CondensationKey::Singular
            }
        })
        .into_iter()
        .for_each(|(key, chunk)| match key {
            CondensationKey::Condensable(_) => {
                let mut condensable_messages =
                    chunk.collect::<Vec<&mut message::MessageDisplay>>();

                let condensed_message = message::condense(
                    &condensable_messages
                        .iter()
                        .map(|message| &**message)
                        .collect::<Vec<&message::MessageDisplay>>(),
                    &buffer_config.server_messages.condense,
                );

                condensable_messages
                    .iter_mut()
                    .for_each(|message| message.condensed = None);

                if let Some(first_message) = condensable_messages.first_mut() {
                    first_message.condensed = condensed_message;
                }
            }
            CondensationKey::Singular => chunk
                .collect::<Vec<&mut message::MessageDisplay>>()
                .iter_mut()
                .for_each(|message| message.condensed = None),
        });
}

/// Backfill previews for replies for messages in a history batch
fn populate_messages_reply_previews(messages: &mut [message::MessageDisplay]) {
    let position_pairs: Vec<(usize, usize)> = messages
        .iter()
        .enumerate()
        .filter_map(|(message_position, message)| {
            message.reply_to.as_ref().and_then(|reply_id| {
                position_reply_target(messages, reply_id, &message.server_time)
                    .map(|reply_target_position| {
                        (message_position, reply_target_position)
                    })
            })
        })
        .collect();

    for (message_position, reply_target_position) in position_pairs {
        if let Some(reply_preview) = messages
            .get(reply_target_position)
            .map(message::MessageDisplay::as_reply_preview)
            && let Some(message) = messages.get_mut(message_position)
        {
            message.reply_preview = Some(reply_preview);
        }
    }
}
