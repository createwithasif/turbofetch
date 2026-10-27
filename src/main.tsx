import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import ProgressRoute from "./ProgressRoute";
import "./App.css";

// Progress windows load the same page with "#progress=<id>".
const progressId = new URLSearchParams(window.location.hash.slice(1)).get("progress");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{progressId ? <ProgressRoute id={progressId} /> : <App />}</React.StrictMode>,
);
