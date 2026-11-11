#include <sqlite3.h>
#include <stdlib.h>
#include <string.h>

const char *people[] = {"Ada Lovelace", "Alan Turing", "Grace Brewster Murray Hopper", "Ken Thompson",
                        "Linus Torvalds", "Dennis Ritchie", "Barbara Liskov", "Radia Perlman",
                        "Edsger Dijkstra", "Frances Allen", "Donald Knuth", "John Backus"};

struct { char last_visitor[16]; sqlite3_stmt *insert; int visits; } app = {"nobody", NULL, 0};

int main(void) {
    sqlite3 *db;
    sqlite3_open(":memory:", &db);
    sqlite3_exec(db, "CREATE TABLE visits(name TEXT)", 0, 0, 0);
    sqlite3_prepare_v2(db, "INSERT INTO visits VALUES (?)", -1, &app.insert, 0);
    for (int i = 0; i < 3; i++) {
        const char *who = people[arc4random_uniform(12)];
        sqlite3_bind_text(app.insert, 1, who, -1, SQLITE_STATIC);
        sqlite3_step(app.insert);
        sqlite3_reset(app.insert);
        memcpy(app.last_visitor, who, strlen(who) + 1);
        app.visits++;
    }
    return sqlite3_finalize(app.insert);
}
