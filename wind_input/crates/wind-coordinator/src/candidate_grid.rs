//! 候选展开态（多行网格 / 卷轴）：进入与收起、二维移动、按屏滚动、平铺下标换算。
//!
//! 设计见 `docs/design/candidate-grid-expand.md`。整个功能立在一个等式上：
//! **网格的一行就是一页**——高亮行就是 `current_page`，高亮列就是 `selected_index`。
//!
//! 所以页模型的一切原样适用、一行不改：数字键 / `;'` / Alt、Ctrl+数字作用于高亮行，空格上屏
//! 高亮项，扩容与末页放宽照常从翻页那条路触发。这里新增的只有两样东西：
//! - **滚动位置** `State::grid_top`（可见区第一行对应的页）；
//! - **二维导航**：↑↓ 换行保持列号、←→ 线性移动（越过行尾折到下一行，正是既有的
//!   `move_up` / `move_down`）、翻页键一次滚一屏、Esc 收起。
//!
//! # 下标
//!
//! 展开态下发给 UI 的是可见区内逐行平铺的候选，命中矩形的 tag 是平铺下标。宿主（Windows
//! 候选窗、host-render、macOS、Linux）都把它原样回传、不解释，换算回「哪一页、页内第几个」
//! 只在这边做（[`Coordinator::grid_cell`]）。

use crate::coordinator::{Coordinator, State};
use crate::pipeline::ModeKind;
use wind_bridge::handler::KeyEventData;
use wind_ipc::protocol::{MOD_SHIFT, MOD_SHORTCUT};
use wind_keys::keymap;

impl Coordinator {
    /// 展开态的生效可见行数。
    pub(crate) fn grid_rows(&self) -> usize {
        self.rt().config.ui.candidate.grid_rows()
    }

    /// 此刻是否处于展开态：有滚动位置**且有候选**。
    ///
    /// ★ 只看 `grid_top` 不够：上屏 / 回车 / 标点顶屏这一批出口只清候选、不经
    /// `reset_candidate_view`，`grid_top` 会留到下一次装填。展开态专属键要是只认它，组合
    /// 结束后在编辑器里按 ←→ / Esc 都会被吞（Linux / macOS 每个键都进协调器）。下一次装填
    /// 必经 `reset_candidate_view`，残值不会被新候选继承，所以判据收在这里即可。
    pub(crate) fn grid_active(state: &State) -> bool {
        state.grid_top.is_some() && !state.candidates.is_empty()
    }

    /// 当前这批候选能不能展开。
    ///
    /// 只在**进入**时判一次：展开之后条件若变了（比如鼠标点了别的东西），收起交给
    /// `reset_candidate_view` 那条统一出口——候选一重装就收。
    pub(crate) fn grid_eligible(&self, state: &State) -> bool {
        if !self.rt().config.ui.candidate.grid_expand {
            return false;
        }
        if state.candidates.is_empty() || state.assoc_active() {
            return false;
        }
        // 范围见设计 §9：快捷输入的数字透镜与格式表依赖单页、辅助码本身就在缩小候选、
        // 特殊模式各有自己的码表语义，都不展开。
        if !matches!(
            state.active,
            None | Some(ModeKind::TempPinyin) | Some(ModeKind::TempEnglish)
        ) {
            return false;
        }
        // 没有第二页（已加载的或还能加载的）就没什么可展开的。
        if self.total_pages(state) < 2 && !state.has_more {
            return false;
        }
        // 只做横排：竖排 / 旋转态的自然形态是「多列并排」，另立项。
        let o = self.desired_orientation(state);
        if o.vertical || o.text != wind_config::TextOrientation::Normal {
            return false;
        }
        // 宿主自绘候选（TSF UI-less）或候选窗被压住时，本地根本不弹窗，展开无从谈起。
        if self.ui_suppressed_by_host().is_some() || self.candidate_window_hidden() {
            return false;
        }
        true
    }

    /// 收起态按翻页键：**只展开、不翻页**（高亮原地不动），下面几页直接出现在视野里。
    ///
    /// 不顺手翻到第 2 页，是因为展开后第 2 页本来就在眼前；翻过去等于让高亮跳过用户
    /// 正要看的那一行（威注音同样如此）。返回是否展开了。
    pub(crate) fn try_expand_grid(&self, state: &mut State) -> bool {
        if Self::grid_active(state) || !self.grid_eligible(state) {
            return false;
        }
        state.grid_top = Some(state.current_page);
        self.clear_hover();
        self.grid_ensure_loaded(state);
        // 在靠后的页上展开（收起态已经翻到了末页附近）：可见区往前让，凑满一屏，
        // 别只露出一行真候选、下面全是占位行。
        let full_top = self.total_pages(state).saturating_sub(self.grid_rows());
        state.grid_top = Some(state.current_page.min(full_top));
        true
    }

    /// 收起回单行。返回是否真的收起了（本就没展开 → `false`）。
    pub(crate) fn collapse_grid(&self, state: &mut State) -> bool {
        if state.grid_top.take().is_none() {
            return false;
        }
        // 悬停下标是平铺下标，收起后对应的已经是另一个格子。
        self.clear_hover();
        true
    }

    /// 让高亮行留在可见区内。页码变动之后都要调（换行、滚屏、线性移动折行、夹页）。
    pub(crate) fn grid_follow(&self, state: &mut State) {
        let Some(top) = state.grid_top else {
            return;
        };
        let rows = self.grid_rows();
        let cur = state.current_page;
        let top = if cur < top {
            cur
        } else if cur >= top + rows {
            cur + 1 - rows
        } else {
            top
        };
        state.grid_top = Some(top);
    }

    /// 可见区之后再预留一屏，不够就向引擎扩容。
    ///
    /// 扩容复用 `expand_candidates`（limit 翻倍、整份重建、保持页码与高亮）——与收起态
    /// `page_next` 「接近末页先扩容」是同一个动作，只是展开态一屏能看见好几页，要提前更多。
    fn grid_ensure_loaded(&self, state: &mut State) {
        let Some(top) = state.grid_top else {
            return;
        };
        if state.has_more && top + 2 * self.grid_rows() >= self.total_pages(state) {
            self.expand_candidates(state);
        }
    }

    /// 换行后把高亮落到第 `col` 列；目标行较短（末页）时落到行尾。
    fn grid_land_column(&self, state: &mut State, col: usize) {
        let (s, e) = self.page_range(state);
        state.selected_index = col.min((e - s).saturating_sub(1));
    }

    /// ↑↓：换行并保持列号。在第一页那一行按 ↑ 收起。返回是否有变化。
    pub(crate) fn grid_move_row(&self, state: &mut State, down: bool) -> bool {
        self.clear_hover();
        let col = state.selected_index;
        if down {
            self.grid_ensure_loaded(state);
            if state.current_page + 1 >= self.total_pages(state) {
                return false;
            }
            Self::turn_page(state, state.current_page + 1);
        } else if state.current_page == 0 {
            return self.collapse_grid(state);
        } else {
            Self::turn_page(state, state.current_page - 1);
        }
        self.grid_land_column(state, col);
        self.grid_follow(state);
        true
    }

    /// 翻页键：一次滚一屏（可见行数那么多行），高亮随之移动并保持列号。
    ///
    /// 已在最后一行再往下翻，交给末页放宽——与收起态「末页按翻页键」是同一条出路，
    /// 放宽出来的候选追加在末尾，照常滚进来。
    pub(crate) fn grid_page(&self, state: &mut State, down: bool) -> bool {
        self.clear_hover();
        let Some(top) = state.grid_top else {
            return false;
        };
        let rows = self.grid_rows();
        let col = state.selected_index;
        let (page, new_top) = if down {
            self.grid_ensure_loaded(state);
            let last = self.total_pages(state) - 1;
            if state.current_page >= last {
                if !self.try_relax_scope_on_page_end(state) {
                    return false;
                }
                self.grid_land_column(state, col);
                self.grid_follow(state);
                return true;
            }
            // 末屏不留空行：最多滚到「最后一页正好落在可见区最后一行」。`.max(top)` 防的是
            // 列表变短后（扩容滤掉了一组生僻字）这个上限比现位置还靠前，那时不往回滚。
            let new_top = (top + rows).min(last.saturating_sub(rows - 1)).max(top);
            ((state.current_page + rows).min(last), new_top)
        } else {
            if state.current_page == 0 {
                return false;
            }
            (
                state.current_page.saturating_sub(rows),
                top.saturating_sub(rows),
            )
        };
        Self::turn_page(state, page);
        state.grid_top = Some(new_top);
        self.grid_land_column(state, col);
        self.grid_follow(state);
        true
    }

    /// 鼠标往下滚 / 点 ›：滚一行；已在最后一行时走末页放宽，与收起态 `page_next` 同一条出路
    /// （同一个 › 不能展开前会放宽、展开后不会）。
    pub(crate) fn grid_scroll_down(&self, state: &mut State) -> bool {
        if self.grid_move_row(state, true) {
            return true;
        }
        let col = state.selected_index;
        if !self.try_relax_scope_on_page_end(state) {
            return false;
        }
        self.grid_land_column(state, col);
        self.grid_follow(state);
        true
    }

    /// ←→：线性移动，越过行尾折到下一行行首、越过行首折到上一行行尾。
    ///
    /// 这正是既有 `move_down` / `move_up` 的「页尾回卷到下一页」——一行就是一页，
    /// 所以不用另写。
    fn grid_move_linear(&self, state: &mut State, forward: bool) -> bool {
        let changed = if forward {
            self.move_down(state)
        } else {
            self.move_up(state)
        };
        self.grid_follow(state);
        changed
    }

    /// 展开态专属的按键：←→ 行内移动、Esc 收起。返回 `Some(是否有变化)` 表示键已归网格，
    /// `None` 表示不是这几个键（或没展开），调用方照常分派。
    ///
    /// 挂在 `apply_session_action` 的最前面：主输入与各 overlay 都先经过它，再落到各自的
    /// 光标移动臂与 Esc 臂——于是展开态下 ←→ 不再移动编码区光标、Esc 只收起不取消，
    /// 而且只需接这一处。
    pub(crate) fn grid_key(&self, state: &mut State, data: &KeyEventData) -> Option<bool> {
        if !Self::grid_active(state) {
            return None;
        }
        if data.modifiers & (MOD_SHIFT | MOD_SHORTCUT) != 0 {
            return None;
        }
        match data.key_code {
            keymap::VK_LEFT => Some(self.grid_move_linear(state, false)),
            keymap::VK_RIGHT => Some(self.grid_move_linear(state, true)),
            // Esc 两段式（设计 D4）：先收起、保留组合；收起态再按一次才是取消。
            keymap::VK_ESCAPE => Some(self.collapse_grid(state)),
            _ => None,
        }
    }

    /// 可见区覆盖的候选绝对下标范围 `[start, end)`；没展开返回 `None`。
    pub(crate) fn grid_visible_range(&self, state: &State) -> Option<(usize, usize)> {
        let top = state.grid_top?;
        let pp = self.per_page(state.active);
        let len = state.candidates.len();
        let start = (top * pp).min(len);
        let end = ((top + self.grid_rows()) * pp).min(len);
        Some((start, end))
    }

    /// 按序号选词 / 按序号操作（数字键、`;'`、Alt / Ctrl+数字）的下标范围 `[start, end)`。
    ///
    /// 收起态就是当页。展开态起点仍是高亮行行首（于是一切 `start + offset` 的写法原样成立），
    /// 终点往后延到带序号的最后一格——序号顺延到下一行时，数字键要跟着选得到那里。
    pub(crate) fn select_range(&self, state: &State) -> (usize, usize) {
        let (start, end) = self.page_range(state);
        match self.grid_visible_range(state) {
            Some((_, vis_end)) if Self::grid_active(state) => (
                start,
                (start + self.grid_index_span(state)).min(vis_end).max(end),
            ),
            _ => (start, end),
        }
    }

    /// 展开态下从高亮行行首起有几格带序号（`ui.candidate.grid_index_fill`），不超出可见区。
    fn grid_index_span(&self, state: &State) -> usize {
        // 数字键 1–9、0 一共十个。
        const DIGITS: usize = 10;
        let pp = self.per_page(state.active);
        let top = state.grid_top.unwrap_or(state.current_page);
        let rows_below = (top + self.grid_rows())
            .saturating_sub(state.current_page)
            .max(1);
        match self.rt().config.ui.candidate.grid_index_fill.as_str() {
            "off" => pp,
            "cells" => DIGITS.max(pp).min(rows_below * pp),
            // "rows" 及未知值：只补整行，避免出现半行带号、半行不带号。
            _ => (DIGITS / pp).max(1).min(rows_below) * pp,
        }
    }

    /// 平铺下标 → `(页, 页内下标)`。没展开、或下标落在可见区外返回 `None`。
    pub(crate) fn grid_cell(&self, state: &State, flat: usize) -> Option<(usize, usize)> {
        let (start, end) = self.grid_visible_range(state)?;
        if start + flat >= end {
            return None;
        }
        let pp = self.per_page(state.active);
        let abs = start + flat;
        Some((abs / pp, abs % pp))
    }

    /// UI 反向事件带来的候选下标 → 页内下标。
    ///
    /// 展开态下先把高亮切到那一格（点哪格、右键哪格，操作的就是哪格），再按页内下标走原有
    /// 逻辑——右键菜单、词条操作、点选上屏都不必知道网格的存在。收起态原样返回。
    /// `redraw`：切了高亮要不要立刻重绘（点选上屏马上就要收窗，不必多画一帧）。
    pub(crate) fn resolve_event_index(&self, idx: usize, redraw: bool) -> Option<usize> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !Self::grid_active(&state) {
            return Some(idx);
        }
        let (page, col) = self.grid_cell(&state, idx)?;
        if page != state.current_page || col != state.selected_index {
            Self::turn_page(&mut state, page);
            state.selected_index = col;
            if redraw {
                self.notify_ui_update(&state);
            }
        }
        Some(col)
    }
}

#[cfg(test)]
mod tests {
    //! 展开态的行为测试。合成候选的几组只走导航（不碰引擎），真实词库那组走完整输入链。
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc::Receiver;
    use wind_bridge::handler::{KeyAction, MessageHandler};
    use wind_ui_types::{CandidateGrid, UiCommand, UiEvent};

    /// 出厂每页 7 个、可见 5 行。
    const PP: usize = 7;
    const ROWS: usize = 5;

    fn key(key_code: u32) -> KeyEventData {
        KeyEventData {
            key_code,
            scan_code: 0,
            modifiers: 0,
            event_type: wind_ipc::protocol::EVENT_KEY_DOWN,
            toggles: 0,
            event_seq: 0,
            prev_char: 0,
        }
    }

    fn coord(grid_on: bool) -> (Arc<Coordinator>, Receiver<UiCommand>) {
        let mut cfg = wind_config::Config::default();
        cfg.input.default.chinese_mode = true;
        cfg.ui.candidate.grid_expand = grid_on;
        let (c, rx) = Coordinator::new_headless_with_ui(cfg, None);
        c.debug_mark_coords_ready();
        (c, rx)
    }

    /// 造 `n` 条候选并挂上一段编码（有编码才有输入会话，Esc 等键才归输入法）。
    fn seed(c: &Coordinator, n: usize) {
        let mut st = c.state.lock().unwrap();
        st.input_buffer = "yi".into();
        st.candidates = (0..n)
            .map(|i| wind_candidate::Candidate {
                text: format!("c{i}"),
                ..Default::default()
            })
            .collect();
        c.reset_candidate_view(&mut st);
    }

    /// (当前页, 页内高亮, 网格首行)。
    fn view(c: &Coordinator) -> (usize, usize, Option<usize>) {
        let st = c.state.lock().unwrap();
        (st.current_page, st.selected_index, st.grid_top)
    }

    /// 最后一帧候选：(文字, 选中, 网格版式)。
    fn last_frame(rx: &Receiver<UiCommand>) -> Option<(Vec<String>, usize, Option<CandidateGrid>)> {
        rx.try_iter()
            .filter_map(|cmd| match cmd {
                UiCommand::UpdateCandidates {
                    candidates,
                    selected,
                    grid,
                    ..
                } => Some((
                    candidates.into_iter().map(|i| i.text).collect(),
                    selected,
                    grid,
                )),
                _ => None,
            })
            .last()
    }

    /// 反向对照：开关关着时翻页键就是翻页，网格不出现。
    #[test]
    fn page_key_just_pages_when_grid_is_off() {
        let (c, _rx) = coord(false);
        seed(&c, PP * 3);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        assert_eq!(view(&c), (1, 0, None));
    }

    /// 第一次按翻页键只展开、不翻页（D3）；下发的帧是可见区内逐行平铺的几页。
    #[test]
    fn first_page_key_expands_without_turning_the_page() {
        let (c, rx) = coord(true);
        seed(&c, PP * 8);
        let _ = rx.try_iter().count();
        assert!(matches!(
            c.handle_key_event(&key(keymap::VK_EQUAL)),
            KeyAction::Consumed
        ));
        assert_eq!(view(&c), (0, 0, Some(0)), "高亮不动，只是展开");
        let (texts, selected, grid) = last_frame(&rx).expect("展开要重绘");
        assert_eq!(texts.len(), PP * ROWS, "一屏 = 可见行数 × 每页数");
        assert_eq!(texts[PP], "c7", "第二行就是第二页");
        assert_eq!(selected, 0);
        assert_eq!(
            grid,
            Some(CandidateGrid {
                cols: PP as u16,
                rows: ROWS as u16,
                highlight_row: 0,
                index_start: 0,
                index_count: PP as u16,
            }),
            "每页 7 个按整行补只编得下高亮行"
        );
    }

    /// 只有一页时没什么可展开：翻页键回到原有语义（翻不动）。
    #[test]
    fn single_page_does_not_expand() {
        let (c, _rx) = coord(true);
        seed(&c, PP);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        assert_eq!(view(&c).2, None);
    }

    /// ↓ 换行并保持列号；← → 线性移动、越过行尾折到下一行。
    #[test]
    fn arrows_move_in_two_dimensions() {
        let (c, rx) = coord(true);
        seed(&c, PP * 8);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        c.handle_key_event(&key(keymap::VK_RIGHT));
        c.handle_key_event(&key(keymap::VK_RIGHT));
        assert_eq!(view(&c), (0, 2, Some(0)));
        c.handle_key_event(&key(keymap::VK_DOWN));
        assert_eq!(view(&c), (1, 2, Some(0)), "换行保持列号");
        let (_, selected, grid) = last_frame(&rx).unwrap();
        assert_eq!(selected, PP + 2, "选中项是平铺下标");
        assert_eq!(grid.unwrap().highlight_row, 1);
        for _ in 0..(PP - 2) {
            c.handle_key_event(&key(keymap::VK_RIGHT));
        }
        assert_eq!(view(&c), (2, 0, Some(0)), "越过行尾折到下一行行首");
        c.handle_key_event(&key(keymap::VK_LEFT));
        assert_eq!(view(&c), (1, PP - 1, Some(0)), "越过行首折回上一行行尾");
    }

    /// 展开态下 ←→ 归网格，不再移动编码区光标。
    #[test]
    fn arrows_do_not_move_the_preedit_caret_when_expanded() {
        let (c, _rx) = coord(true);
        seed(&c, PP * 3);
        c.state.lock().unwrap().input_cursor_pos = 2;
        c.handle_key_event(&key(keymap::VK_EQUAL));
        c.handle_key_event(&key(keymap::VK_LEFT));
        assert_eq!(c.state.lock().unwrap().input_cursor_pos, 2);
    }

    /// 翻页键一次滚一屏，高亮随之下移一屏并保持列号；末屏不留空行。
    #[test]
    fn page_key_scrolls_a_screen_when_expanded() {
        let (c, _rx) = coord(true);
        seed(&c, PP * 8); // 8 页
        c.handle_key_event(&key(keymap::VK_EQUAL));
        c.handle_key_event(&key(keymap::VK_RIGHT));
        c.handle_key_event(&key(keymap::VK_EQUAL));
        // 末页是第 7 页：可见区最多滚到 3..=7，高亮从 0 下移 5 行到第 5 页。
        assert_eq!(view(&c), (5, 1, Some(3)));
        c.handle_key_event(&key(keymap::VK_EQUAL));
        assert_eq!(view(&c), (7, 1, Some(3)), "到底后高亮落到末行");
        c.handle_key_event(&key(keymap::VK_MINUS));
        assert_eq!(view(&c), (2, 1, Some(0)));
        c.handle_key_event(&key(keymap::VK_MINUS));
        assert_eq!(view(&c), (0, 1, Some(0)), "首屏再往上翻回到首行，不收起");
        assert!(matches!(
            c.handle_key_event(&key(keymap::VK_MINUS)),
            KeyAction::Consumed
        ));
        assert_eq!(view(&c).2, Some(0));
    }

    /// 高亮行滚出可见区时可见区跟随。
    #[test]
    fn visible_rows_follow_the_highlight() {
        let (c, rx) = coord(true);
        seed(&c, PP * 8);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        for _ in 0..ROWS {
            c.handle_key_event(&key(keymap::VK_DOWN));
        }
        assert_eq!(view(&c), (5, 0, Some(1)));
        let (texts, selected, grid) = last_frame(&rx).unwrap();
        assert_eq!(texts[0], "c7", "可见区第一行是第 1 页");
        assert_eq!(grid.unwrap().highlight_row as usize, ROWS - 1);
        assert_eq!(selected, (ROWS - 1) * PP);
    }

    /// 末屏行数不足一屏：照常下发实有的那些，补行由渲染端负责。
    #[test]
    fn short_last_screen_sends_only_real_rows() {
        let (c, rx) = coord(true);
        seed(&c, PP * 2 + 3);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        let (texts, _, grid) = last_frame(&rx).unwrap();
        assert_eq!(texts.len(), PP * 2 + 3);
        assert_eq!(grid.unwrap().rows as usize, ROWS);
    }

    /// ↓ 到末行后停住；↑ 在第一行收起。
    #[test]
    fn up_on_first_row_collapses() {
        let (c, rx) = coord(true);
        seed(&c, PP * 3);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        c.handle_key_event(&key(keymap::VK_DOWN));
        c.handle_key_event(&key(keymap::VK_DOWN));
        c.handle_key_event(&key(keymap::VK_DOWN));
        assert_eq!(view(&c).0, 2, "末行再按 ↓ 停住");
        c.handle_key_event(&key(keymap::VK_UP));
        c.handle_key_event(&key(keymap::VK_UP));
        assert_eq!(view(&c), (0, 0, Some(0)));
        c.handle_key_event(&key(keymap::VK_UP));
        assert_eq!(view(&c).2, None, "第一行按 ↑ 收起");
        let (texts, _, grid) = last_frame(&rx).unwrap();
        assert_eq!((texts.len(), grid), (PP, None), "收起后回到单行");
    }

    /// Esc 两段式（D4）：先收起、保留编码；再按才取消。
    #[test]
    fn escape_collapses_first_then_cancels() {
        let (c, _rx) = coord(true);
        seed(&c, PP * 3);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        c.handle_key_event(&key(keymap::VK_DOWN));
        assert!(matches!(
            c.handle_key_event(&key(keymap::VK_ESCAPE)),
            KeyAction::Consumed
        ));
        {
            let st = c.state.lock().unwrap();
            assert_eq!(st.grid_top, None);
            assert_eq!(st.input_buffer, "yi", "收起不动编码");
            assert_eq!(st.current_page, 1, "收起停在高亮所在的那一页");
        }
        assert!(matches!(
            c.handle_key_event(&key(keymap::VK_ESCAPE)),
            KeyAction::ClearComposition
        ));
    }

    /// 回车上屏这类出口只清候选、不经视图复位：组合结束后 ←→ / Esc 必须交还宿主，
    /// 不能被残留的展开态吞掉（Linux / macOS 每个键都会进协调器）。
    #[test]
    fn keys_go_back_to_the_host_after_the_composition_ends() {
        let (c, _rx) = coord(true);
        seed(&c, PP * 3);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        assert!(view(&c).2.is_some());
        c.handle_key_event(&key(keymap::VK_RETURN));
        assert!(
            c.state.lock().unwrap().candidates.is_empty(),
            "回车上屏了编码"
        );
        for vk in [keymap::VK_LEFT, keymap::VK_RIGHT, keymap::VK_ESCAPE] {
            assert!(
                matches!(c.handle_key_event(&key(vk)), KeyAction::PassThrough),
                "组合已结束，键 {vk:#x} 应交还宿主"
            );
        }
    }

    /// 在靠后的页上展开：可见区往前让、凑满一屏，而不是只露出一行真候选。
    #[test]
    fn expanding_near_the_end_fills_the_screen() {
        let (c, rx) = coord(true);
        seed(&c, PP * 6);
        for _ in 0..(PP * 4) {
            c.handle_key_event(&key(keymap::VK_DOWN)); // 收起态逐个移动，回卷到第 4 页
        }
        assert_eq!(view(&c), (4, 0, None));
        c.handle_key_event(&key(keymap::VK_EQUAL));
        assert_eq!(view(&c), (4, 0, Some(1)), "6 页可见 5 行 ⇒ 首行是第 1 页");
        let (texts, selected, grid) = last_frame(&rx).unwrap();
        assert_eq!(texts.len(), PP * ROWS);
        assert_eq!(grid.unwrap().highlight_row, 3);
        assert_eq!(selected, 3 * PP);
    }

    /// UI 回传平铺下标：右键第 3 行第 2 格，高亮先切过去，菜单目标是那一格。
    #[test]
    fn flat_index_from_ui_lands_on_that_cell() {
        let (c, _rx) = coord(true);
        seed(&c, PP * 8);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        assert_eq!(c.resolve_event_index(2 * PP + 1, true), Some(1));
        assert_eq!(view(&c), (2, 1, Some(0)));
        assert_eq!(c.resolve_event_index(PP * ROWS, true), None, "可见区外");
        // 收起态原样返回（快捷键等调用方传的就是页内下标）。
        c.collapse_grid(&mut c.state.lock().unwrap());
        assert_eq!(c.resolve_event_index(3, true), Some(3));
    }

    /// 悬停校验按整个可见区，不按单页：第 2 行的格子也是合法悬停目标。
    #[test]
    fn hover_accepts_cells_beyond_the_first_row() {
        let (c, _rx) = coord(true);
        seed(&c, PP * 3);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        c.handle_ui_event(UiEvent::Hover((PP + 1) as i32));
        assert_eq!(c.hover_target(), (PP + 1) as i32);
    }

    /// 滚轮 / 翻页箭头在展开态滚一行；往上滚到顶不收起。收起态往下滚先展开。
    #[test]
    fn mouse_page_scrolls_rows() {
        let (c, _rx) = coord(true);
        seed(&c, PP * 3);
        c.handle_ui_event(UiEvent::Page(1));
        assert_eq!(view(&c), (0, 0, Some(0)), "收起态往下滚 = 展开");
        c.handle_ui_event(UiEvent::Page(1));
        assert_eq!(view(&c).0, 1);
        c.handle_ui_event(UiEvent::Page(-1));
        c.handle_ui_event(UiEvent::Page(-1));
        assert_eq!(view(&c), (0, 0, Some(0)), "滚到顶停住");
    }

    /// 只有横排展开：竖排下翻页键照旧翻页。
    #[test]
    fn vertical_layout_does_not_expand() {
        let mut cfg = wind_config::Config::default();
        cfg.input.default.chinese_mode = true;
        cfg.ui.candidate.grid_expand = true;
        cfg.ui.candidate.layout = "vertical".into();
        let (c, _rx) = Coordinator::new_headless_with_ui(cfg, None);
        seed(&c, PP * 3);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        assert_eq!(view(&c), (1, 0, None));
    }

    /// 自定每页数与序号顺延档位的协调器。
    fn coord_with(per_page: usize, fill: &str) -> (Arc<Coordinator>, Receiver<UiCommand>) {
        let mut cfg = wind_config::Config::default();
        cfg.input.default.chinese_mode = true;
        cfg.ui.candidate.grid_expand = true;
        cfg.ui.candidate.per_page = per_page;
        cfg.ui.candidate.grid_index_fill = fill.into();
        let (c, rx) = Coordinator::new_headless_with_ui(cfg, None);
        c.debug_mark_coords_ready();
        (c, rx)
    }

    /// 最后一帧里带序号的格子：(平铺下标起点, 格数, 各格序号)。
    fn index_frame(rx: &Receiver<UiCommand>) -> (u16, u16, Vec<String>) {
        rx.try_iter()
            .filter_map(|cmd| match cmd {
                UiCommand::UpdateCandidates {
                    candidates,
                    grid: Some(g),
                    ..
                } => Some((
                    g.index_start,
                    g.index_count,
                    candidates.into_iter().map(|i| i.label).collect::<Vec<_>>(),
                )),
                _ => None,
            })
            .last()
            .expect("应下发展开态的帧")
    }

    /// 只补整行（出厂）：每页 5 个时高亮行 1–5、下一行 6–10（第 10 个由数字键 0 选，序号沿用
    /// 普通候选窗的写法），数字键的选择范围跟着延到下一行。
    #[test]
    fn indices_continue_onto_whole_rows() {
        let (c, rx) = coord_with(5, "rows");
        seed(&c, 5 * 8);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        c.handle_key_event(&key(keymap::VK_DOWN));
        let (from, count, labels) = index_frame(&rx);
        assert_eq!((from, count), (5, 10), "高亮在第 2 行：第 2、3 行带号");
        assert_eq!(
            &labels[5..15],
            &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]
        );
        let st = c.state.lock().unwrap();
        assert_eq!(c.select_range(&st), (5, 15), "数字键的范围跟着序号走");
    }

    /// 高亮在可见区最后一行：下面没有可见行可补，只编高亮行（不给看不见的格子编号）。
    #[test]
    fn indices_stay_in_the_visible_rows() {
        let (c, rx) = coord_with(5, "rows");
        seed(&c, 5 * 8);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        for _ in 0..(ROWS - 1) {
            c.handle_key_event(&key(keymap::VK_DOWN));
        }
        let (from, count, _) = index_frame(&rx);
        assert_eq!((from, count), ((ROWS as u16 - 1) * 5, 5));
    }

    /// 逐格档：每页 7 个时编满 10 个，下一行前 3 格带 8、9、10；off 档只编高亮行。
    #[test]
    fn index_fill_cells_and_off() {
        let (c, rx) = coord_with(7, "cells");
        seed(&c, 7 * 4);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        let (from, count, labels) = index_frame(&rx);
        assert_eq!((from, count), (0, 10));
        assert_eq!(&labels[7..10], &["8", "9", "10"]);
        let (c, rx) = coord_with(5, "off");
        seed(&c, 5 * 4);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        assert_eq!(index_frame(&rx).1, 5);
    }

    /// 菜单开着时在另一格上右键：高亮与悬停都跟到新的这一格（悬停若停在旧格，旧格的悬停
    /// 底色会留在原处，看上去像高亮没跟过来）。
    #[test]
    fn second_right_click_moves_highlight_and_hover() {
        let (c, rx) = coord(true);
        seed(&c, PP * 3);
        c.handle_key_event(&key(keymap::VK_EQUAL));
        for idx in [PP + 2, 2 * PP + 4] {
            c.handle_ui_event(UiEvent::RequestCandidateMenu {
                page_local: idx,
                x: 0,
                y: 0,
            });
        }
        assert_eq!(c.hover_target(), (2 * PP + 4) as i32);
        let (_, selected, _) = last_frame(&rx).unwrap();
        assert_eq!(selected, 2 * PP + 4);
    }

    fn data_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
    }

    /// 真实词库：打 `yi`、展开、↓ 到第二行、按 2 ⇒ 上屏的是第 2 页第 2 个。
    /// 数字键语义没动（作用于高亮行 = 当前页），这条把它钉在完整输入链上。
    #[test]
    fn number_key_selects_in_the_highlighted_row() {
        if !data_dir().join("schemas/pinyin.schema.toml").exists() {
            return;
        }
        let mut cfg = wind_config::Config::default();
        cfg.schema.available = vec!["pinyin".into()];
        cfg.schema.active = "pinyin".into();
        cfg.input.default.chinese_mode = true;
        cfg.ui.candidate.grid_expand = true;
        let (c, rx) = Coordinator::new_headless_with_ui(cfg, Some(&data_dir()));
        c.debug_mark_coords_ready();
        for ch in "YI".chars() {
            c.handle_key_event(&key(ch as u32));
        }
        c.handle_key_event(&key(keymap::VK_EQUAL));
        c.handle_key_event(&key(keymap::VK_DOWN));
        let (texts, selected, _) = last_frame(&rx).expect("展开态应下发候选");
        assert_eq!(selected, PP);
        let want = c.state.lock().unwrap().candidates[PP + 1].text.clone();
        assert_eq!(texts[PP + 1], want);
        match c.handle_key_event(&key(u32::from(b'2'))) {
            KeyAction::InsertText { text, .. } => assert_eq!(text, want),
            other => panic!("按 2 应上屏第 2 页第 2 个，实得 {other:?}"),
        }
        assert_eq!(c.state.lock().unwrap().grid_top, None, "上屏后收起");
    }

    /// 真实词库、每页 5 个：展开后按 6 上屏的是第二行行首（序号顺延到下一行，数字键跟着走）。
    #[test]
    fn number_key_follows_indices_onto_the_next_row() {
        if !data_dir().join("schemas/pinyin.schema.toml").exists() {
            return;
        }
        let mut cfg = wind_config::Config::default();
        cfg.schema.available = vec!["pinyin".into()];
        cfg.schema.active = "pinyin".into();
        cfg.input.default.chinese_mode = true;
        cfg.ui.candidate.grid_expand = true;
        cfg.ui.candidate.per_page = 5;
        let (c, _rx) = Coordinator::new_headless_with_ui(cfg, Some(&data_dir()));
        c.debug_mark_coords_ready();
        for ch in "YI".chars() {
            c.handle_key_event(&key(ch as u32));
        }
        c.handle_key_event(&key(keymap::VK_EQUAL));
        let want = c.state.lock().unwrap().candidates[5].text.clone();
        match c.handle_key_event(&key(u32::from(b'6'))) {
            KeyAction::InsertText { text, .. } => assert_eq!(text, want),
            other => panic!("按 6 应上屏第二行行首，实得 {other:?}"),
        }
    }
}
