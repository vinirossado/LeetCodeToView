package com.code2complexity.api.web;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.List;

/**
 * `language`/`code` carry the ACTUAL source that was submitted for this
 * execution (see {@code Execution#getLanguage()}/{@code getCode()}) — added
 * so a page reload mid-execution (or opening a shared link) can restore the
 * real submitted code+language into the editor instead of leaving whatever
 * starter example happened to be showing. Before this, GET /trace only
 * returned events, so the frontend had no way to know what code actually
 * produced the reconnected trace and silently kept showing the starter
 * example next to live/real variable values it could never have produced —
 * see frontend's app.ts (ExecutionSessionService.restoredCode/restoredLanguage).
 */
public record TraceResponse(String executionId, String status, String language, String code, List<JsonNode> events) {
}
