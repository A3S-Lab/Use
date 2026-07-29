"use strict";

document.querySelector("#status").addEventListener("click", async () => {
  const response = await fetch("/api/status", { method: "GET" });
  const result = document.querySelector("#result");
  result.textContent = response.ok ? "Index ready" : "Index unavailable";
});
