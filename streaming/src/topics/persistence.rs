use crate::partitions::partition::Partition;
use crate::topics::topic::Topic;
use ringbuffer::RingBufferWrite;
use shared::error::Error;
use std::path::Path;
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, trace};

impl Topic {
    pub async fn load(&mut self) -> Result<(), Error> {
        info!("Loading topic with ID: {} from disk...", &self.id);
        if !Path::new(&self.path).exists() {
            return Err(Error::TopicNotFound(self.id));
        }

        let topic_info_file = OpenOptions::new().read(true).open(&self.info_path).await;
        if topic_info_file.is_err() {
            return Err(Error::CannotOpenTopicInfo(self.id));
        }

        let mut topic_info = String::new();
        if topic_info_file
            .unwrap()
            .read_to_string(&mut topic_info)
            .await
            .is_err()
        {
            return Err(Error::CannotReadTopicInfo(self.id));
        }

        self.name = topic_info;
        let dir_files = fs::read_dir(&self.path).await;
        if dir_files.is_err() {
            return Err(Error::CannotReadPartitions(self.id));
        }

        let mut dir_files = dir_files.unwrap();
        loop {
            let dir_entry = dir_files.next_entry().await;
            if dir_entry.is_err() {
                break;
            }

            let dir_entry = dir_entry.unwrap();
            if dir_entry.is_none() {
                break;
            }

            let dir_entry = dir_entry.unwrap();
            let metadata = dir_entry.metadata().await;
            if metadata.is_err() || metadata.unwrap().is_file() {
                continue;
            }

            let id = dir_entry
                .file_name()
                .to_str()
                .unwrap()
                .parse::<u32>()
                .unwrap();
            let mut partition =
                Partition::create(id, &self.path, false, self.config.partition.clone());
            partition.load().await?;
            self.partitions.insert(partition.id, partition);
        }

        self.load_messages_to_cache().await?;

        info!(
            "Loaded topic: '{}' with ID: {} from disk.",
            &self.name, &self.id
        );

        Ok(())
    }

    pub async fn persist(&mut self) -> Result<(), Error> {
        if Path::new(&self.path).exists() {
            return Err(Error::TopicAlreadyExists(self.id));
        }

        if std::fs::create_dir(&self.path).is_err() {
            return Err(Error::CannotCreateTopicDirectory(self.id));
        }

        info!("Topic with ID {} was saved, path: {}", self.id, self.path);

        let topic_info_file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&self.info_path)
            .await;

        if topic_info_file.is_err() {
            return Err(Error::CannotCreateTopicInfo(self.id));
        }

        if topic_info_file
            .unwrap()
            .write_all(self.name.as_bytes())
            .await
            .is_err()
        {
            return Err(Error::CannotUpdateTopicInfo(self.id));
        }

        info!(
            "Creating {} partition(s) for topic with ID: {}...",
            self.partitions.len(),
            &self.id
        );
        for partition in self.partitions.iter_mut() {
            if std::fs::create_dir(&partition.1.path).is_err() {
                return Err(Error::CannotCreatePartitionDirectory(*partition.0, self.id));
            }

            partition.1.persist().await?;
            info!(
                "Partition with ID {} for topic with ID: {} was saved, path: {}",
                partition.0, &self.id, partition.1.path
            );
        }

        Ok(())
    }

    pub async fn persist_messages(&mut self) -> Result<(), Error> {
        for partition in self.get_partitions_mut() {
            for segment in partition.get_segments_mut() {
                segment.persist_messages().await?;
            }
        }

        Ok(())
    }

    pub async fn delete(&self) -> Result<(), Error> {
        info!("Deleting topic with ID: {}...", &self.id);
        if fs::remove_dir_all(&self.path).await.is_err() {
            return Err(Error::CannotDeleteTopicDirectory(self.id));
        }

        info!("Deleted topic with ID: {}.", &self.id);

        Ok(())
    }

    async fn load_messages_to_cache(&mut self) -> Result<(), Error> {
        let messages_buffer_size = self.config.partition.messages_buffer as u64;
        if messages_buffer_size == 0 {
            return Ok(());
        }

        for (_, partition) in self.partitions.iter_mut() {
            if partition.segments.is_empty() {
                trace!("No segments found for partition ID: {}", partition.id);
                continue;
            }

            let end_offset = partition.segments.last().unwrap().current_offset;
            let start_offset = if end_offset + 1 >= messages_buffer_size {
                end_offset + 1 - messages_buffer_size
            } else {
                0
            };

            let messages_count = (end_offset - start_offset + 1) as u32;
            trace!(
                "Loading {} messages for partition ID: {} from offset: {} to offset: {}...",
                messages_count,
                partition.id,
                start_offset,
                end_offset
            );

            let messages = partition.get_messages(start_offset, messages_count).await?;
            for message in messages {
                partition.messages.push(message);
            }

            trace!(
                "Loaded {} messages for partition ID: {} from offset: {} to offset: {}.",
                messages_count,
                partition.id,
                start_offset,
                end_offset
            );
        }

        Ok(())
    }
}