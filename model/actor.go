package model

// Different status types
type Status struct {
	// TO DO
}

// Everything is an entity
type Entity struct {
	Name        string
	Full_hp     int
	C_hp        int
	Phy_def     int
	Phy_atk     int
	Speed       int
	Coordinates Cords
	Destination Cords
}

// Actors will be things that can attack or be attacked...
type Actor interface {
	ReduceHP(int)
	IncreaseHP(int)
	IsDead() bool
	GetStatuses() []Status
	Attack() Actor
	Move(Cords)
	GetCurrentHP() int
	GetMaxHP() int
	GetDefense() int
	GetAttack() int
	GetLocation() Cords
}
