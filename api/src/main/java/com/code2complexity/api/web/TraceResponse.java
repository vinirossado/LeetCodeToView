package com.code2complexity.api.web;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.List;

public record TraceResponse(String executionId, String status, List<JsonNode> events) {
}
