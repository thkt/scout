# Domain Documentation

Generate domain model documentation for developers working with this codebase.

## Sections

### 1. Glossary

Table with columns: Term | Definition | Context | Source

Extract key domain terms from code: type names, module names, constants.
Define each term in plain language. Include the source file where it's defined.

### 2. Entities

For each core data type / model / struct:

- **Name** and source file reference
- **Fields**: table with Name | Type | Nullable | Description | Source columns
- **Invariants**: list of rules that must always hold

Where to find invariants:

- Constructor validation or `new()` guards
- `validate()` / `is_valid()` methods
- Database constraints (UNIQUE, CHECK, NOT NULL)
- Type-level refinements (newtypes, enums)
- If none are found, omit the Invariants subsection

Include all entity types regardless of persistence mechanism (ORM models, plain structs, database tables, protocol buffers).

### 3. Value Objects

For types that represent values without identity (e.g., Email, Money, DateRange):

- **Name**, source file, and which entities use it
- **Fields**: table with Name | Type | Nullable | Source columns

Omit this section if no value objects are found.

### 4. Relationships

Mermaid ER diagram showing entity relationships.

Table with columns: From | To | Type | Description | Source

### 5. Business Rules

Table with columns: Rule | Description | Enforced By | Source

List validation rules, constraints, and domain logic.
Omit this section if fewer than 2 rules are found.

### 6. Domain Events

Table with columns: Event | Trigger | Subscribers | Source

List events, hooks, callbacks, or message patterns.
Omit this section if no event patterns are found.

## Analysis Techniques

1. **Data modeling approach**: identify the ORM, schema tool, or plain type definitions used in the project
2. **Schema discovery**: Glob for model/entity/schema directories, then read full files. Look for both framework-specific patterns and plain struct/class definitions
3. **Exhaustive field extraction**: read full schema files. Every field in the output must trace to a `file:line` from an actual Read
4. **Nullable detection**: identify the language's nullable/optional type pattern and document each field's nullability accurately
5. **Domain logic discovery**: Grep for service, use-case, policy, and handler patterns by name and directory convention
6. **Relationship extraction**: trace foreign keys, relation attributes, and type references between entities

## Writing Guidelines

- Write for a developer who needs to understand the data model
- Focus on the domain concepts, not the framework/ORM specifics
- Use `file_path:line_number` references for all source citations
- When updating, verify each `file_path:line_number` reference is still accurate

## Omit Rules

- Omit Value Objects if none are found
- Omit Business Rules if fewer than 2 rules exist
- Omit Domain Events if no event patterns exist
- Never omit Glossary, Entities, or Relationships — these are the core of domain documentation
