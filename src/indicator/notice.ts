import { CircleAlert, Info } from "lucide";
import { createIcon } from "../lib/icons";
import type { IndicatorNoticeKind } from "../lib/types";
import type { ActiveNotice } from "./state";

const NOTICE_ICONS: Record<IndicatorNoticeKind, string> = {
  info: createIcon(Info),
  error: createIcon(CircleAlert),
};

export interface NoticeViewElements {
  icon: HTMLElement;
  root: HTMLElement;
  text: HTMLElement;
}

/** Renders the notice face (icon + single-line message) and owns its timer. */
export class NoticeView {
  private readonly elements: NoticeViewElements;
  private renderedId: number | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(elements: NoticeViewElements) {
    this.elements = elements;
  }

  /** Fills in the notice content; a no-op if it is already shown. */
  render(notice: ActiveNotice): void {
    if (notice.id === this.renderedId) {
      return;
    }
    this.renderedId = notice.id;
    const { icon, root, text } = this.elements;
    root.dataset.kind = notice.kind;
    // Static, trusted SVG markup; the message itself only goes through
    // textContent.
    icon.innerHTML = NOTICE_ICONS[notice.kind];
    text.textContent = notice.message;
  }

  /**
   * Natural width of the notice content in layout px. Uses offsetWidth so the
   * pill's scale transform (e.g. while scaled out) does not affect it.
   */
  measureWidth(): number {
    return this.elements.root.offsetWidth;
  }

  /** (Re)starts the expiry timer; a newer notice replaces the pending one. */
  scheduleExpiry(notice: ActiveNotice, onExpire: (id: number) => void): void {
    this.cancelExpiry();
    this.timer = setTimeout(() => {
      this.timer = null;
      onExpire(notice.id);
    }, notice.durationMs);
  }

  cancelExpiry(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
}
