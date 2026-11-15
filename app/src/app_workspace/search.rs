use super::*;

impl LanvoxApp {
    pub fn load_inbox(&mut self, mentions_only: bool, before: Option<MessageId>) {
        if !self.client.as_ref().is_some_and(|client| client.is_alive()) {
            return;
        }
        self.workspace_ui.inbox_requested = Some(mentions_only);
        self.workspace_ui.inbox_before = before;
        self.workspace_ui.inbox_loading = true;
        self.workspace_ui.inbox_dirty = false;
        self.workspace_ui.inbox_error = None;
        if before.is_none() {
            self.workspace_ui.inbox_results.clear();
            self.workspace_ui.inbox_has_more = false;
        }
        self.workspace(WorkspaceCommand::Inbox {
            mentions_only,
            before,
            limit: 50,
        });
    }
    pub fn search_messages(&mut self, mut request: SearchRequest) {
        request.query = request
            .query
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        request.offset = 0;
        request.limit = 50;
        self.workspace_ui.search_request = request.clone();
        self.workspace_ui.search_submitted = request.clone();
        self.workspace_ui.search_open = true;
        self.workspace_ui.search_loading = true;
        self.workspace_ui.search_error = None;
        self.workspace_ui.search_results.clear();
        self.workspace_ui.search_query = request.query.clone();
        self.workspace_ui.search_offset = 0;
        self.workspace_ui.search_has_more = false;
        self.workspace(WorkspaceCommand::SearchAdvanced(request));
    }

    pub fn begin_search(&mut self, request: SearchRequest) {
        self.search_messages(request);
    }

    pub fn next_search_page(&mut self) {
        if self.workspace_ui.search_loading || !self.workspace_ui.search_has_more {
            return;
        }
        let mut request = self.workspace_ui.search_submitted.clone();
        request.offset =
            self.workspace_ui.search_offset + self.workspace_ui.search_results.len() as u32;
        self.workspace_ui.search_loading = true;
        self.workspace(WorkspaceCommand::SearchAdvanced(request));
    }

    pub(super) fn accept_search_page(
        &mut self,
        query: String,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        offset: u32,
        has_more: bool,
    ) {
        if query != self.workspace_ui.search_submitted.query {
            return;
        }
        self.workspace_ui.search_results = messages;
        self.workspace_ui.search_query = query;
        self.workspace_ui.search_open = true;
        self.workspace_ui.search_loading = false;
        self.workspace_ui.search_offset = offset;
        self.workspace_ui.search_has_more = has_more;
        if let Some(view) = &mut self.view {
            for detail in details {
                view.workspace
                    .details
                    .retain(|d| d.message != detail.message);
                view.workspace.details.push(detail);
            }
        }
    }
}
