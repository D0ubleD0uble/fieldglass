// The width the render panel draws an image at (#1252), for the tests that
// check it.
//
// The panel caps every image at 80% of the page's height times its aspect, plus
// the canvas's 2 px border (`.canvas-wrap` in render-panel.ts), so the width a
// test expects depends on the window's height and on how much room the row
// gives: a tall, narrow editor leaves the row as the limit, a wide, short one
// leaves the cap.

/** The border box of an image whose width over height is `aspect`, in a page
 *  `viewportHeight` tall, in a row with `room` px from the image's left edge
 *  to the row's right edge. */
export function expectedImageWidth(viewportHeight: number, aspect: number, room: number): number {
  return Math.min(room, 0.8 * viewportHeight * aspect + 2);
}
