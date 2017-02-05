package entity

type Status struct {
    // TO DO
}

type Coordinates struct{
    X,Y int
}

type Actor interface {
    ReduceHP(int)
    IncreaseHP(int)
    IsDead() bool
    GetStatuses() []Status
    Attack() Actor
    Move(Coordinates)
    GetCurrentHP() int
    GetMaxHP() int
    GetDefense() int
    GetAttack() int
    GetLocation() Coordinates
}

