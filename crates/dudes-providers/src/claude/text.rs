use dudes_proto::providers::claude::{ApiMessage, ContentBlock};
use dudes_types::SessionId;
use serde::Deserialize;

use super::{Line, StreamLine, assistant_text};
use crate::stream::{TextBlock, TextFragment, TextLayout, TextProgress};

#[derive(Debug, Default)]
pub(super) struct LiveText {
    message: Option<MessageText>,
    layout: TextLayout,
}

#[derive(Debug)]
struct MessageText {
    id: String,
    phase: MessagePhase,
    blocks: TextProgress,
    block_index: Option<usize>,
}

#[derive(Debug, PartialEq, Eq)]
enum MessagePhase {
    Streaming,
    Stopped,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PartialMessage {
    parent_tool_use_id: Option<String>,
    #[serde(rename = "isSynthetic")]
    is_synthetic: Option<bool>,
    event: Option<StreamEvent>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct StreamEvent {
    #[serde(rename = "type")]
    kind: String,
    message: Option<ApiMessage>,
    index: Option<usize>,
    content_block: Option<ContentBlock>,
    delta: Option<Delta>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Delta {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

impl LiveText {
    pub(super) fn take_omitted_text(&mut self) -> bool {
        self.message
            .as_mut()
            .is_some_and(|message| message.blocks.take_omitted_text())
    }

    pub(super) fn read(
        &mut self,
        line: &Line,
        announced: Option<&SessionId>,
    ) -> Result<Vec<TextBlock>, serde_json::Error> {
        let session = line.session_id().filter(|id| !id.is_empty());
        if announced.is_some_and(|id| session != Some(id.as_str())) {
            return Ok(Vec::new());
        }
        let Some(session) = session else {
            return Ok(Vec::new());
        };
        match &line.body {
            StreamLine::Unknown { raw } if line.envelope.kind == "stream_event" => {
                let partial = PartialMessage::deserialize(raw)?;
                if announced.is_none()
                    || partial.parent_tool_use_id.is_some()
                    || partial.is_synthetic == Some(true)
                {
                    return Ok(Vec::new());
                }
                Ok(partial
                    .event
                    .as_ref()
                    .and_then(|event| self.partial(event))
                    .map(|text| TextBlock {
                        session: SessionId::new(session),
                        text: self.layout.render(text),
                    })
                    .into_iter()
                    .collect())
            }
            StreamLine::Assistant(message) if assistant_text(line).next().is_some() => {
                let streamed = self
                    .message
                    .as_mut()
                    .filter(|streamed| message.message.id.as_deref() == Some(streamed.id.as_str()));
                let Some(streamed) = streamed else {
                    return Ok(assistant_text(line)
                        .map(|(_, text)| TextBlock {
                            session: SessionId::new(session),
                            text: self.layout.render(TextFragment::BlockStart(text)),
                        })
                        .collect());
                };
                let offset = if message.message.content.len() == 1 {
                    streamed.block_index.unwrap_or_default()
                } else {
                    0
                };
                Ok(message
                    .message
                    .content
                    .iter()
                    .enumerate()
                    .filter(|(_, block)| block.kind == "text")
                    .filter_map(|(index, block)| {
                        streamed
                            .blocks
                            .complete(offset + index, block.text.as_deref()?)
                    })
                    .map(|text| TextBlock {
                        session: SessionId::new(session),
                        text: self.layout.render(text),
                    })
                    .collect())
            }
            StreamLine::Result(_) => {
                self.message = None;
                Ok(Vec::new())
            }
            StreamLine::System { subtype, .. }
                if matches!(subtype.as_deref(), Some("compact_boundary" | "api_retry")) =>
            {
                self.message = None;
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn partial<'a>(&mut self, event: &'a StreamEvent) -> Option<TextFragment<'a>> {
        if event.kind == "message_start" {
            self.message = None;
            let message = event.message.as_ref()?;
            self.message = message
                .id
                .as_ref()
                .filter(|id| !id.is_empty() && message.role == "assistant")
                .map(|id| MessageText {
                    id: id.clone(),
                    phase: MessagePhase::Streaming,
                    blocks: TextProgress::default(),
                    block_index: None,
                });
            return None;
        }
        let message = self.message.as_mut()?;
        if event.kind == "message_stop" {
            message.phase = MessagePhase::Stopped;
            return None;
        }
        if message.phase != MessagePhase::Streaming {
            return None;
        }
        let index = event.index?;
        match event.kind.as_str() {
            "content_block_start" => {
                let block = event.content_block.as_ref()?;
                message.block_index = Some(index);
                message.blocks.start(index, block.kind == "text");
                message
                    .blocks
                    .delta(index, block.text.as_deref().unwrap_or_default())
            }
            "content_block_delta" => {
                let delta = event
                    .delta
                    .as_ref()
                    .filter(|delta| delta.kind == "text_delta")?;
                message.blocks.delta(index, delta.text.as_deref()?)
            }
            "content_block_stop" => {
                message.blocks.stop(index);
                None
            }
            _ => None,
        }
    }
}
