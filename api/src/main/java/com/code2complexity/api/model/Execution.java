package com.code2complexity.api.model;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.ArrayList;
import java.util.List;

// Mutation (status, events) is only ever done through ExecutionStore, which
// owns the lock that makes it safe to read/write across threads.
public final class Execution {
    private final String id;
    private final String language;
    private final String code;
    private volatile ExecutionStatus status = ExecutionStatus.PENDING;
    private final List<JsonNode> events = new ArrayList<>();

    public Execution(String id, String language, String code) {
        this.id = id;
        this.language = language;
        this.code = code;
    }

    public String getId() {
        return id;
    }

    public String getLanguage() {
        return language;
    }

    public String getCode() {
        return code;
    }

    public ExecutionStatus getStatus() {
        return status;
    }

    public void setStatus(ExecutionStatus status) {
        this.status = status;
    }

    public List<JsonNode> getEvents() {
        return events;
    }
}
