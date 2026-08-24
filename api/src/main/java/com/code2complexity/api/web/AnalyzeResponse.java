package com.code2complexity.api.web;

import com.fasterxml.jackson.databind.JsonNode;

public record AnalyzeResponse(JsonNode methods) {
}
