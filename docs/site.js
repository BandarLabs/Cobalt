"use strict";

// The tour is a recording of the reader's own screen, shown inside a drawn
// reader. A browser's player bar would sit on top of the very screen being
// shown, so the page offers one plain control beneath it instead. Without
// this script the video keeps the browser's controls and still plays.
const video = document.querySelector("#tour-video");
const control = document.querySelector("#tour-control");
const play = document.querySelector("#tour-play");
if (video && control && play) {
  video.controls = false;
  control.hidden = false;
  const label = () => {
    play.textContent = video.paused ? "Play the tour" : "Pause";
  };
  play.addEventListener("click", () => {
    if (video.paused) {
      video.play().catch(() => {
        video.controls = true;
        control.hidden = true;
      });
    } else {
      video.pause();
    }
  });
  video.addEventListener("play", label);
  video.addEventListener("pause", label);
}
