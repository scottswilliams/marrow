//! The durable value codec: canonical byte forms, the frozen known answers, and the
//! depth bounds the kernel mirrors from the image.

#[path = "codec/encoding.rs"]
mod encoding;
#[path = "codec/image_bounds.rs"]
mod image_bounds;
#[path = "codec/known_answers.rs"]
mod known_answers;
