package com.code2complexity.api.model;

import com.fasterxml.jackson.annotation.JsonValue;

public enum ExecutionStatus {
    PENDING,
    RUNNING,
    COMPLETED,
    FAILED;

    @JsonValue
    public String jsonValue() {
        return name().toLowerCase();
    }
}
