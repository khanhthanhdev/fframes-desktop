import React from "react";
import { Composition, registerRoot } from "remotion";
import { GridVideo, WIDTH, HEIGHT } from "./scene.jsx";

registerRoot(() => (
  <Composition
    id="MixedGrid"
    component={GridVideo}
    width={WIDTH}
    height={HEIGHT}
    fps={30}
    durationInFrames={303}
    defaultProps={{ fontCss: "" }}
  />
));
