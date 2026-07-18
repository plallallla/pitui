use super::*;

pub fn scroll_home(
    In(invocation): In<OperationInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> OperationExecution {
    update_scroll(
        invocation.source_dataset,
        ScrollAction::Home,
        &mut viewports,
    )
}

pub fn scroll_end(
    In(invocation): In<OperationInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> OperationExecution {
    update_scroll(invocation.source_dataset, ScrollAction::End, &mut viewports)
}

pub fn scroll_page_up(
    In(invocation): In<OperationInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> OperationExecution {
    update_scroll(
        invocation.source_dataset,
        ScrollAction::PageUp,
        &mut viewports,
    )
}

pub fn scroll_page_down(
    In(invocation): In<OperationInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> OperationExecution {
    update_scroll(
        invocation.source_dataset,
        ScrollAction::PageDown,
        &mut viewports,
    )
}

#[derive(Clone, Copy)]
enum ScrollAction {
    Home,
    End,
    PageUp,
    PageDown,
}

fn update_scroll(
    dataset: Entity,
    action: ScrollAction,
    viewports: &mut Query<&mut DatasetViewport>,
) -> OperationExecution {
    let Ok(mut viewport) = viewports.get_mut(dataset) else {
        return OperationExecution::Rejected("active Dataset has no text viewport".into());
    };
    let page_size = viewport.page_size.max(1);
    let max_offset = viewport.content_length.saturating_sub(page_size);
    viewport.offset = match action {
        ScrollAction::Home => 0,
        ScrollAction::End => max_offset,
        ScrollAction::PageUp => viewport.offset.saturating_sub(page_size),
        ScrollAction::PageDown => viewport.offset.saturating_add(page_size).min(max_offset),
    };
    OperationExecution::Completed
}
