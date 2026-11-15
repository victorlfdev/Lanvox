use super::*;

impl Store {
    /// Unread inbox over authorized channel/cursor pairs. A keyset cursor keeps paging stable
    /// while new messages arrive. The caller derives the pairs from server-side authorization.
    pub fn inbox(
        &self,
        user: UserId,
        channels: &[(ChannelId, MessageId)],
        mentions_only: bool,
        before: Option<MessageId>,
        limit: usize,
    ) -> io::Result<(Vec<Message>, bool)> {
        let channels = serde_json::to_string(channels).map_err(error)?;
        let limit = limit.clamp(1, SEARCH_MAX_LIMIT);
        let before = before.map(|v| v.min(i64::MAX as u64) as i64);
        let mut statement = self.connection.prepare(
            "SELECT m.body FROM messages m JOIN json_each(?1) c ON m.channel = json_extract(c.value, '$[0]') \
             WHERE m.id > json_extract(c.value, '$[1]') AND m.author <> ?2 \
             AND (?3 = 0 OR EXISTS(SELECT 1 FROM message_mentions x WHERE x.message=m.id AND x.user=?2)) \
             AND (?4 IS NULL OR m.id < ?4) ORDER BY m.id DESC LIMIT ?5"
        ).map_err(error)?;
        let rows = statement
            .query_map(
                params![
                    channels,
                    user,
                    i64::from(mentions_only),
                    before,
                    (limit + 1) as i64
                ],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .map_err(error)?;
        let mut messages: Vec<Message> = rows
            .map(|r| serde_json::from_slice(&r.map_err(error)?).map_err(error))
            .collect::<io::Result<_>>()?;
        let has_more = messages.len() > limit;
        messages.truncate(limit);
        Ok((messages, has_more))
    }
    pub fn search(
        &self,
        channel: ChannelId,
        query: &str,
        limit: usize,
    ) -> io::Result<Vec<Message>> {
        // instr treats wildcard characters literally rather than expanding user input.
        let mut statement=self.connection.prepare("SELECT body FROM messages WHERE channel=?1 AND instr(lower(text),lower(?2))>0 ORDER BY id DESC LIMIT ?3").map_err(error)?;
        let rows = statement
            .query_map(params![channel, query, limit as i64], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .map_err(error)?;
        rows.map(|r| serde_json::from_slice(&r.map_err(error)?).map_err(error))
            .collect()
    }

    /// Indexed search (FTS5 prefix terms) with optional filters, ordering and paging.
    /// `channels` is the set the caller may read; returns the page and whether more follow.
    pub fn search_advanced(
        &self,
        channels: &[ChannelId],
        request: &SearchRequest,
    ) -> io::Result<(Vec<Message>, bool)> {
        let limit = match request.limit as usize {
            0 => SEARCH_DEFAULT_LIMIT,
            n => n.min(SEARCH_MAX_LIMIT),
        };
        let fts = fts_query(&request.query);
        let mut sql = String::from(if fts.is_some() {
            "SELECT m.body FROM messages_fts JOIN messages m ON m.id=messages_fts.rowid WHERE messages_fts MATCH :q AND "
        } else {
            "SELECT m.body FROM messages m WHERE "
        });
        sql.push_str(
            "m.channel IN (SELECT value FROM json_each(:channels)) \
             AND (:author IS NULL OR m.author=:author) \
             AND (:after IS NULL OR m.time>=:after) \
             AND (:before IS NULL OR m.time<=:before) \
             AND (:mention IS NULL OR EXISTS(SELECT 1 FROM message_mentions x WHERE x.user=:mention AND x.message=m.id)) \
             AND (:att IS NULL OR COALESCE((SELECT has_attachment FROM message_meta x WHERE x.id=m.id),0)=:att) ORDER BY ",
        );
        sql.push_str(match (request.order, fts.is_some()) {
            (SearchOrder::Oldest, _) => "m.id ASC",
            (SearchOrder::Relevance, true) => "bm25(messages_fts), m.id DESC",
            _ => "m.id DESC",
        });
        sql.push_str(" LIMIT :limit OFFSET :offset");
        let channels = serde_json::to_string(channels).map_err(error)?;
        let author = request.author;
        let after = request.after;
        let before = request.before;
        let mention = request.mentions;
        let att = request.has_attachment.map(i64::from);
        let fetch = limit as i64 + 1;
        let offset = i64::from(request.offset);
        let mut named: Vec<(&str, &dyn rusqlite::ToSql)> = vec![
            (":channels", &channels),
            (":author", &author),
            (":after", &after),
            (":before", &before),
            (":mention", &mention),
            (":att", &att),
            (":limit", &fetch),
            (":offset", &offset),
        ];
        if let Some(q) = &fts {
            named.push((":q", q));
        }
        let mut statement = self.connection.prepare(&sql).map_err(error)?;
        let rows = statement
            .query_map(named.as_slice(), |r| r.get::<_, Vec<u8>>(0))
            .map_err(error)?;
        let mut out: Vec<Message> = rows
            .map(|r| serde_json::from_slice(&r.map_err(error)?).map_err(error))
            .collect::<io::Result<_>>()?;
        let more = out.len() > limit;
        out.truncate(limit);
        Ok((out, more))
    }
}
