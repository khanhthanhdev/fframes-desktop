import React, { useEffect, useMemo, useState } from "react";
import {
  interpolate,
  staticFile,
  useCurrentFrame,
  useDelayRender,
} from "remotion";

export const WIDTH = 3840;
export const HEIGHT = 2160;

const circles = 99000;
const textNodes = 1000;
const nodes = circles + textNodes;
const panels = 20;
const perPanel = nodes / panels;
const textStep = nodes / textNodes;
const color = (id, frame) =>
  `rgb(${(id * 13 + frame * 17) % 256},${(id * 7 + frame * 29) % 256},${(id * 3 + frame * 43) % 256})`;
function circleRadius(slot, frame) {
  if (slot % 10 !== 1) return 16 + (slot % 4) * 4;
  return interpolate((slot + frame) % 60, [0, 30, 60], [16, 28, 16], {
    easing: t => t * t * (3 - 2 * t),
  });
}
function Cell({ slot }) {
  const frame = useCurrentFrame();
  const id = (slot + frame * 37) % nodes;
  if (slot % textStep === 0) {
    const panel = Math.floor(slot / perPanel);
    const index = (slot % perPanel) / textStep;
    return (
      <text
        key={slot}
        x={(panel % 5) * 200 + 24 + (index % 10) * 16}
        y={Math.floor(panel / 5) * 250 + 40 + Math.floor(index / 10) * 40}
        fontFamily="DM Sans"
        fontSize="16"
        fill="#fff"
      >
        {(id + frame) % 10}
      </text>
    );
  }
  const radius = circleRadius(slot, frame);
  return (
    <circle
      key={slot}
      cx={32 + ((slot * 13 + frame * 3) % 128)}
      cy={32 + ((slot * 17 + frame * 5) % 176)}
      r={radius}
      fill={color(id, frame)}
    />
  );
}
export function GridVideo({ fontCss = "" }) {
  const { delayRender, continueRender, cancelRender } = useDelayRender();
  const [fontHandle] = useState(() =>
    delayRender("benchmark font", { retries: 0 })
  );
  useEffect(() => {
    const font = new FontFace(
      "DM Sans",
      `url(${staticFile("DMSans-Regular.ttf")})`
    );
    font
      .load()
      .then(loaded => {
        document.fonts.add(loaded);
        continueRender(fontHandle);
      })
      .catch(cancelRender);
  }, [fontHandle, continueRender, cancelRender]);
  const { layers, texts } = useMemo(() => {
    const layers = [];
    for (let panel = 0; panel < panels; panel++) {
      const cells = [];
      for (let slot = panel * perPanel; slot < (panel + 1) * perPanel; slot++)
        if (slot % textStep !== 0) cells.push(<Cell key={slot} slot={slot} />);
      layers.push(
        <g
          key={panel}
          transform={`translate(${(panel % 5) * 200} ${Math.floor(panel / 5) * 250})`}
        >
          {cells}
        </g>
      );
    }
    const texts = [];
    for (let slot = 0; slot < nodes; slot += textStep)
      texts.push(<Cell key={slot} slot={slot} />);
    return { layers, texts };
  }, []);
  return (
    <svg
      width={WIDTH}
      height={HEIGHT}
      viewBox="0 0 1000 1000"
      preserveAspectRatio="none"
      style={{ display: "block", background: "#18202c" }}
    >
      <defs>
        <style>{fontCss}</style>
      </defs>
      {layers}
      {texts}
    </svg>
  );
}
